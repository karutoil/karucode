import { getAuthToken } from '../api/client';
import type { TerminalControlFrame } from '../api/types';

/**
 * How long to wait before reconnecting after the socket drops.
 *
 * A reload is an immediate reconnect (the component mounts and attaches), so
 * this delay is only for a drop while the tab is still open: a laptop waking
 * from sleep, a Wi-Fi handover, a server restart. The first retry is quick
 * because the common cause is the server coming back; the ceiling keeps a
 * stopped server from being hammered.
 */
const RECONNECT_DELAYS_MS = [250, 500, 1000, 2000, 5000, 10_000];

/** How long to wait for the shell to answer a ping before treating it as gone. */
const PING_TIMEOUT_MS = 20_000;

export type TerminalSocketStatus =
  | 'idle'
  | 'connecting'
  | 'open'
  | 'reconnecting'
  | 'closed'
  /** The server said no (auth, a disabled terminal, an unknown id): retrying cannot help. */
  | 'failed';

export interface TerminalSocketHandlers {
  /** PTY bytes, already decoded from a binary frame. */
  onData: (bytes: Uint8Array) => void;
  /** A control frame (`ready`, `exit`, `lagged`, `detached`, `error`, `pong`). */
  onControl: (frame: TerminalControlFrame) => void;
  /** Connection state, for the status line in the terminal header. */
  onStatus: (status: TerminalSocketStatus, detail?: string) => void;
}

export interface TerminalSocketOptions {
  /** Terminal id to attach to. */
  id: string;
  /** Window size for the initial PTY, so the replay is wrapped correctly. */
  cols: number;
  rows: number;
  handlers: TerminalSocketHandlers;
  /** Override the websocket factory (tests). */
  createSocket?: (url: string) => WebSocket;
  /** Override the token (tests). */
  token?: string | null;
}

/**
 * The terminal's websocket: one per visible terminal.
 *
 * The URL is built by hand rather than from `apiRequest`, because a browser
 * cannot set an `Authorization` header on a handshake - the token therefore
 * travels as `?token=`, which the server accepts for exactly this route (and
 * `/api/events`). `Origin` is sent by the browser and checked on the server, so
 * a cross-site page cannot open a shell.
 *
 * Reconnection is automatic and **replays**: on attach the server sends the
 * tail of its scrollback buffer as the first binary frame, so a reconnect lands
 * on a terminal that already looks like it did. The local screen is cleared
 * first so the replay is not appended to stale output.
 */
export class TerminalSocket {
  private socket: WebSocket | null = null;
  private status: TerminalSocketStatus = 'idle';
  private attempt = 0;
  private timer: number | null = null;
  private pingTimer: number | null = null;
  private disposed = false;
  private readonly options: TerminalSocketOptions;
  /** The most recent size, so a reconnect starts the new shell at the right one. */
  private cols: number;
  private rows: number;

  constructor(options: TerminalSocketOptions) {
    this.options = options;
    this.cols = options.cols;
    this.rows = options.rows;
  }

  get isOpen(): boolean {
    return this.socket?.readyState === WebSocket.OPEN;
  }

  get currentStatus(): TerminalSocketStatus {
    return this.status;
  }

  /** Attach (or re-attach). Idempotent while already open. */
  connect(): void {
    if (this.disposed || this.isOpen) return;
    this.openSocket();
  }

  private openSocket(): void {
    this.clearTimers();
    this.setStatus(this.attempt === 0 ? 'connecting' : 'reconnecting');
    const url = this.buildUrl();
    const factory = this.options.createSocket ?? ((target: string) => new WebSocket(target));

    let socket: WebSocket;
    try {
      socket = factory(url);
    } catch (error) {
      this.setStatus('failed', error instanceof Error ? error.message : String(error));
      return;
    }
    socket.binaryType = 'arraybuffer';
    this.socket = socket;

    socket.onopen = () => {
      this.attempt = 0;
      this.setStatus('open');
      // Send the real size immediately: the URL carried the size at connect
      // time, and the browser may have been resized between.
      this.post({ type: 'resize', cols: this.cols, rows: this.rows });
      this.startPing();
    };

    socket.onmessage = (event: MessageEvent) => {
      const control = readControlFrame(event.data);
      if (control !== null) {
        this.handleControl(control);
        return;
      }
      const bytes = readBytes(event.data);
      if (bytes) {
        this.options.handlers.onData(bytes);
        return;
      }
      // A `Blob` arrives when `binaryType` was not honoured (an old engine, or a
      // test double); read it and forward once it resolves.
      if (isBlob(event.data)) {
        void event.data.arrayBuffer().then((buffer) => {
          if (!this.disposed) this.options.handlers.onData(new Uint8Array(buffer));
        });
      }
    };

    socket.onclose = (event: CloseEvent) => {
      this.socket = null;
      this.clearTimers();
      if (this.disposed) return;
      // A normal close after an `exit` frame, or a refusal, is final: the tab
      // stays with its "shell exited" state instead of retrying forever.
      if (this.status === 'failed' || event.code === 1000) {
        this.setStatus('closed', event.reason || undefined);
        return;
      }
      this.scheduleReconnect(event.reason || undefined);
    };

    socket.onerror = () => {
      // A browser gives no detail on a websocket error; `onclose` follows and
      // carries the decision about retrying.
      if (this.status !== 'open') this.setStatus('reconnecting');
    };
  }

  private handleControl(raw: string): void {
    let frame: TerminalControlFrame;
    try {
      frame = JSON.parse(raw) as TerminalControlFrame;
    } catch {
      // A text frame that is not JSON is not ours; ignore it rather than
      // writing it to the terminal, which would smear garbage on the screen.
      return;
    }
    if (frame?.type === 'error') {
      this.setStatus('failed', frame.message);
    }
    this.options.handlers.onControl(frame);
  }

  private startPing(): void {
    this.clearPing();
    this.pingTimer = window.setTimeout(() => {
      // No pong within the window means the socket is dead on a network the
      // browser has not noticed yet (a laptop that changed network). Closing it
      // makes `onclose` fire and the reconnect path run.
      this.socket?.close(4000, 'no response');
    }, PING_TIMEOUT_MS);
  }

  private clearPing(): void {
    if (this.pingTimer !== null) {
      window.clearTimeout(this.pingTimer);
      this.pingTimer = null;
    }
  }

  private scheduleReconnect(detail?: string): void {
    if (this.disposed) return;
    const delay = RECONNECT_DELAYS_MS[Math.min(this.attempt, RECONNECT_DELAYS_MS.length - 1)];
    this.attempt += 1;
    this.setStatus('reconnecting', detail);
    this.timer = window.setTimeout(() => {
      this.timer = null;
      this.openSocket();
    }, delay);
  }

  /** Send raw keystrokes as a binary frame. */
  send(bytes: Uint8Array): void {
    if (!this.isOpen) return;
    this.socket?.send(bytes);
  }

  /** Tell the shell the window changed size. */
  resize(cols: number, rows: number): void {
    if (cols === this.cols && rows === this.rows) return;
    this.cols = cols;
    this.rows = rows;
    this.post({ type: 'resize', cols, rows });
  }

  /** Re-open the connection (the **Reconnect** button after an exit). */
  reconnect(): void {
    if (this.disposed) return;
    this.attempt = 0;
    this.clearTimers();
    this.closeSocket();
    this.openSocket();
  }

  private post(value: Record<string, unknown>): void {
    if (!this.isOpen) return;
    this.socket?.send(JSON.stringify(value));
  }

  private buildUrl(): string {
    const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const params = new URLSearchParams({
      cols: String(Math.round(this.cols)),
      rows: String(Math.round(this.rows)),
    });
    const token = this.options.token !== undefined ? this.options.token : getAuthToken();
    if (token) params.set('token', token);
    return `${scheme}//${window.location.host}/api/terminals/${encodeURIComponent(this.options.id)}/socket?${params}`;
  }

  private setStatus(status: TerminalSocketStatus, detail?: string): void {
    this.status = status;
    this.options.handlers.onStatus(status, detail);
  }

  private closeSocket(): void {
    const socket = this.socket;
    this.socket = null;
    if (!socket) return;
    socket.onopen = null;
    socket.onmessage = null;
    socket.onclose = null;
    socket.onerror = null;
    try {
      socket.close(1000, 'detach');
    } catch {
      // Already closing; nothing to do.
    }
  }

  private clearTimers(): void {
    if (this.timer !== null) {
      window.clearTimeout(this.timer);
      this.timer = null;
    }
    this.clearPing();
  }

  /** Detach and never reconnect. Called when the terminal unmounts. */
  dispose(): void {
    this.disposed = true;
    this.clearTimers();
    this.closeSocket();
    this.setStatus('idle');
  }
}

/**
 * A binary websocket frame as bytes.
 *
 * `instanceof ArrayBuffer` is deliberately not used: across realms (a jsdom
 * test double, an iframe, some embedding contexts) the constructor differs and
 * `instanceof` is false for a perfectly good buffer. The byte length and the
 * constructor name are what actually identify an `ArrayBuffer` here.
 */
function readBytes(data: unknown): Uint8Array | null {
  if (!isArrayBuffer(data)) return null;
  return new Uint8Array(data);
}

function isArrayBuffer(value: unknown): value is ArrayBuffer {
  if (value === null || typeof value !== 'object') return false;
  const candidate = value as ArrayBuffer & { byteLength?: unknown; slice?: unknown };
  return (
    typeof candidate.byteLength === 'number' &&
    typeof candidate.slice === 'function' &&
    // An ArrayBuffer has no `BYTES_PER_ELEMENT`; a typed array does.
    !('BYTES_PER_ELEMENT' in (value as object))
  );
}

function isBlob(value: unknown): value is Blob {
  return (
    value !== null &&
    typeof value === 'object' &&
    typeof (value as Blob).arrayBuffer === 'function' &&
    typeof (value as Blob).size === 'number'
  );
}

/** The JSON control frame a text message carries, if it is one. */
function readControlFrame(data: unknown): string | null {
  return typeof data === 'string' ? data : null;
}