import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TerminalSocket, type TerminalSocketStatus } from './terminalSocket';
import type { TerminalControlFrame } from '../api/types';

/**
 * A stand-in for the browser's `WebSocket`.
 *
 * jsdom has no `WebSocket` that can talk to a server, so the socket is driven by
 * hand: the test decides when it opens, what it receives and when it closes,
 * which is exactly the sequence the reconnect and replay logic depends on.
 */
class FakeSocket {
  static readonly OPEN = 1;
  static readonly CLOSED = 3;
  readyState = 0;
  binaryType = 'blob';
  sent: (string | Uint8Array)[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: { code: number; reason: string }) => void) | null = null;
  onerror: (() => void) | null = null;

  constructor(readonly url: string) {}

  send(data: string | Uint8Array): void {
    this.sent.push(data);
  }

  close(code = 1000, reason = ''): void {
    this.readyState = FakeSocket.CLOSED;
    this.onclose?.({ code, reason });
  }

  /** Simulate the server accepting the upgrade. */
  open(): void {
    this.readyState = FakeSocket.OPEN;
    this.onopen?.();
  }

  /** Simulate a binary frame from the PTY. */
  binary(bytes: Uint8Array): void {
    this.onmessage?.({ data: bytes.buffer });
  }

  /** Simulate a JSON control frame. */
  control(frame: TerminalControlFrame): void {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }

  /** Simulate the socket dropping without a clean close. */
  drop(code = 1006, reason = 'network'): void {
    this.readyState = FakeSocket.CLOSED;
    this.onclose?.({ code, reason });
  }
}

function harness(options: { token?: string | null } = {}) {
  const sockets: FakeSocket[] = [];
  const data: Uint8Array[] = [];
  const controls: TerminalControlFrame[] = [];
  const statuses: TerminalSocketStatus[] = [];

  // `WebSocket.OPEN` is read by `isOpen`.
  (globalThis as unknown as { WebSocket: unknown }).WebSocket = FakeSocket;

  const socket = new TerminalSocket({
    id: 't_abc',
    cols: 100,
    rows: 30,
    token: options.token === undefined ? null : options.token,
    createSocket: (url) => {
      const created = new FakeSocket(url);
      sockets.push(created);
      return created as unknown as WebSocket;
    },
    handlers: {
      onData: (bytes) => data.push(bytes),
      onControl: (frame) => controls.push(frame),
      onStatus: (status) => statuses.push(status),
    },
  });

  return { socket, sockets, data, controls, statuses };
}

describe('TerminalSocket', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('builds the documented url with the window size and the token', () => {
    const { socket, sockets } = harness({ token: 'tok-123' });
    socket.connect();
    expect(sockets).toHaveLength(1);
    const url = new URL(sockets[0]!.url);
    expect(url.pathname).toBe('/api/terminals/t_abc/socket');
    expect(url.searchParams.get('cols')).toBe('100');
    expect(url.searchParams.get('rows')).toBe('30');
    // A browser cannot set a header on a handshake, so the token is a query
    // parameter - exactly like `/api/events`.
    expect(url.searchParams.get('token')).toBe('tok-123');
  });

  it('omits the token when there is none', () => {
    const { socket, sockets } = harness({ token: null });
    socket.connect();
    expect(new URL(sockets[0]!.url).searchParams.has('token')).toBe(false);
  });

  it('forwards binary frames as bytes and text frames as control', () => {
    const { socket, sockets, data, controls } = harness();
    socket.connect();
    sockets[0]!.open();
    sockets[0]!.binary(new Uint8Array([104, 105]));
    sockets[0]!.control({ type: 'exit', code: 0 });
    expect(Array.from(data[0]!)).toEqual([104, 105]);
    expect(controls).toEqual([{ type: 'exit', code: 0 }]);
  });

  it('ignores a text frame that is not JSON instead of writing it on screen', () => {
    const { socket, sockets, data } = harness();
    socket.connect();
    sockets[0]!.open();
    sockets[0]!.onmessage?.({ data: 'not json at all' });
    // Smearing a non-JSON text frame onto the terminal would corrupt the screen.
    expect(data).toHaveLength(0);
  });

  it('sends keystrokes as binary and a resize as a control frame', () => {
    const { socket, sockets } = harness();
    socket.connect();
    sockets[0]!.open();
    socket.send(new TextEncoder().encode('ls\r'));
    socket.resize(120, 40);
    const sent = sockets[0]!.sent;
    const text = sent.filter((item): item is string => typeof item === 'string');
    const binary = sent.filter((item) => typeof item !== 'string') as Uint8Array[];
    // The initial resize on open, then the explicit one.
    const resizes = text.map((item) => JSON.parse(item) as { type: string; cols: number; rows: number });
    expect(resizes.map((frame) => [frame.type, frame.cols, frame.rows])).toEqual([
      ['resize', 100, 30],
      ['resize', 120, 40],
    ]);
    // Keystrokes travel as raw bytes, not as a JSON string: the PTY expects
    // exactly what the keyboard produced.
    expect(binary).toHaveLength(1);
    expect(Array.from(binary[0]!)).toEqual(Array.from(new TextEncoder().encode('ls\r')));
  });

  it('does not send a resize that changed nothing', () => {
    const { socket, sockets } = harness();
    socket.connect();
    sockets[0]!.open();
    const before = sockets[0]!.sent.length;
    socket.resize(100, 30);
    expect(sockets[0]!.sent).toHaveLength(before);
  });

  it('reconnects after a drop, and the replay is delivered as data', () => {
    const { socket, sockets, data, statuses } = harness();
    socket.connect();
    sockets[0]!.open();
    // A clean server close with the run still alive: the tab reconnects.
    sockets[0]!.drop(1006, 'network');
    expect(statuses).toContain('reconnecting');

    vi.advanceTimersByTime(250);
    expect(sockets).toHaveLength(2);
    sockets[1]!.open();
    // The server replays the scrollback as the first binary frame.
    sockets[1]!.control({
      type: 'ready',
      replay: true,
      replay_bytes: 5,
      pid: 42,
      running: true,
    });
    sockets[1]!.binary(new TextEncoder().encode('$ ls\r'));
    expect(new TextDecoder().decode(data[0]!)).toBe('$ ls\r');
  });

  it('backs off the reconnect rather than hammering a dead server', () => {
    const { socket, sockets } = harness();
    socket.connect();
    sockets[0]!.open();
    // Three drops in a row walk the delay ladder.
    sockets[0]!.drop();
    vi.advanceTimersByTime(250);
    expect(sockets).toHaveLength(2);
    sockets[1]!.drop();
    vi.advanceTimersByTime(499);
    expect(sockets).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(3);
    sockets[2]!.drop();
    vi.advanceTimersByTime(999);
    expect(sockets).toHaveLength(3);
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(4);
  });

  it('does not reconnect after a clean close or a server refusal', () => {
    const { socket, sockets, statuses } = harness();
    socket.connect();
    sockets[0]!.open();
    // A server-sent error is final: retrying would just be refused again.
    sockets[0]!.control({ type: 'error', message: 'no such terminal' });
    sockets[0]!.close(1000, 'done');
    vi.advanceTimersByTime(30_000);
    expect(sockets).toHaveLength(1);
    expect(statuses).toContain('failed');
  });

  it('stops for good once disposed', () => {
    const { socket, sockets } = harness();
    socket.connect();
    sockets[0]!.open();
    socket.dispose();
    vi.advanceTimersByTime(30_000);
    expect(sockets).toHaveLength(1);
    expect(socket.currentStatus).toBe('idle');
  });

  it('reconnect() opens a new socket immediately', () => {
    const { socket, sockets } = harness();
    socket.connect();
    sockets[0]!.open();
    socket.reconnect();
    expect(sockets).toHaveLength(2);
  });

  it('closes a socket that never answers a ping', () => {
    const { socket, sockets } = harness();
    socket.connect();
    sockets[0]!.open();
    // No pong: the watchdog closes, which triggers the reconnect path.
    vi.advanceTimersByTime(20_000);
    expect(sockets[0]!.readyState).toBe(FakeSocket.CLOSED);
  });
});