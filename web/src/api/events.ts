import type { ConnectionState, RawEvent } from './types';

export interface EventStreamStatusDetail {
  attempt: number;
  error?: string;
}

/** A hole in `seq`: the server skipped frames this client never received. */
export interface EventStreamGap {
  /** Highest `seq` seen before the hole (0 when the connection had none yet). */
  after: number;
  /** The `seq` that revealed the hole. */
  seq: number;
  /** Frames missing between the two, by sequence arithmetic. */
  missed: number;
}

export interface EventStreamOptions {
  /** Defaults to `/api/events` (same origin, or proxied by Vite). */
  url?: string;
  token?: string | null;
  fetchImpl?: typeof fetch;
  onEvent: (event: RawEvent) => void;
  onStatus?: (state: ConnectionState, detail?: EventStreamStatusDetail) => void;
  /** Called after a connection is established (including reconnects). */
  onOpen?: () => void;
  /**
   * Called when the stream jumps forward in `seq`, i.e. frames were dropped
   * (server-side lag, a stalled proxy hop, a half-open reconnect). The listener
   * is expected to refetch the session when replay cannot repair the hole.
   * Only meaningful for an unfiltered stream: a `?session=` filter makes holes
   * normal.
   */
  onGap?: (gap: EventStreamGap) => void;
  /** The server cannot replay the requested cursor; refetch authoritative state. */
  onResync?: () => void;
  minDelayMs?: number;
  maxDelayMs?: number;
  /**
   * Reconnect after this long without a single byte (event or `: keepalive`).
   * The server sends a keepalive every 15 s, so the default tolerates three
   * misses before declaring the connection dead.
   */
  idleTimeoutMs?: number;
  random?: () => number;
  sleep?: (ms: number, signal: AbortSignal) => Promise<void>;
}

export interface EventStreamHandle {
  stop: () => void;
  getState: () => ConnectionState;
}

function defaultSleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) {
      resolve();
      return;
    }
    const timer = setTimeout(finish, ms);
    signal.addEventListener('abort', finish, { once: true });
    function finish(): void {
      clearTimeout(timer);
      signal.removeEventListener('abort', finish);
      resolve();
    }
  });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

type IdleRead =
  | { kind: 'chunk'; value: Uint8Array | undefined; done: boolean }
  | { kind: 'error'; error: unknown }
  | { kind: 'idle' }
  | { kind: 'aborted' };

/** How long a teardown is allowed to take before the loop stops waiting for it. */
const CANCEL_GRACE_MS = 1_000;

/** "45s" / "250ms", so a short watchdog does not print "0s" in an error. */
function describeMs(ms: number): string {
  return ms >= 1000 ? `${Math.round(ms / 1000)}s` : `${ms}ms`;
}

/**
 * One `reader.read()`, but never wait forever: a half-open connection (sleep,
 * Wi-Fi change, proxy or server restart without a FIN) leaves the read pending
 * indefinitely, which is how a live UI goes silently stale. Resolves `'idle'`
 * when nothing at all arrived within `idleMs` (0 or less disables the watchdog),
 * `'aborted'` when the caller stopped the stream, and leaves no timer behind.
 */
function readWithIdleTimeout(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  idleMs: number,
  signal: AbortSignal,
): Promise<IdleRead> {
  return new Promise<IdleRead>((resolve) => {
    let settled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const finish = (result: IdleRead): void => {
      if (settled) return;
      settled = true;
      if (timer !== undefined) clearTimeout(timer);
      signal.removeEventListener('abort', onAbort);
      resolve(result);
    };
    function onAbort(): void {
      finish({ kind: 'aborted' });
    }
    if (idleMs > 0) {
      timer = setTimeout(() => finish({ kind: 'idle' }), idleMs);
    }
    if (signal.aborted) {
      finish({ kind: 'aborted' });
      return;
    }
    signal.addEventListener('abort', onAbort, { once: true });
    reader.read().then(
      (result) => finish({ kind: 'chunk', value: result.value, done: result.done }),
      // Keep the reason: it is the only diagnostic for a proxy reset mid-stream.
      (error: unknown) => finish({ kind: 'error', error }),
    );
  });
}

/**
 * Release the body without waiting on anything we do not control: a `cancel()`
 * that never settles would wedge the reconnect loop exactly like the dead socket
 * the idle watchdog exists to catch.
 */
async function cancelReader(reader: ReadableStreamDefaultReader<Uint8Array>): Promise<void> {
  try {
    await Promise.race([
      reader.cancel(),
      new Promise<void>((resolve) => {
        setTimeout(resolve, CANCEL_GRACE_MS);
      }),
    ]);
  } catch {
    // The connection is already gone; nothing to clean up.
  }
}

/** Validates the envelope fields we rely on; unknown event types are accepted. */
export function parseEventPayload(payload: string): RawEvent | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(payload) as unknown;
  } catch {
    return null;
  }
  if (!isRecord(parsed)) return null;
  if (typeof parsed.seq !== 'number' || !Number.isFinite(parsed.seq)) return null;
  if (typeof parsed.ts_ms !== 'number' || !Number.isFinite(parsed.ts_ms)) return null;
  if (typeof parsed.type !== 'string' || parsed.type === '') return null;
  return parsed as RawEvent;
}

const TRANSPORT_CONTROL_RECOVERY = {
  /** Capacity refusal lost no events: retry from the existing cursor. */
  connection_cap: 'retry',
  /** The rejected frame and any disconnect-window frames need authoritative repair. */
  stream_overflow: 'resync',
} as const;

export type EventStreamTransportControlType = keyof typeof TRANSPORT_CONTROL_RECOVERY;

export interface EventStreamTransportControl {
  type: EventStreamTransportControlType;
  message: string;
  retry_after_ms?: number;
}

function isTransportControlType(value: unknown): value is EventStreamTransportControlType {
  return typeof value === 'string' && Object.hasOwn(TRANSPORT_CONTROL_RECOVERY, value);
}

export interface SseFrame {
  /** The SSE `id:` line, which is the resume cursor (not a JSON data field). */
  id: string | null;
  /** Normal runtime envelope; null for a non-replayable transport control. */
  event: RawEvent | null;
  /** `event: karucode.transport`: deliberately outside the event-sequence cursor. */
  transport?: EventStreamTransportControl;
}

function parseTransportControl(payload: string): EventStreamTransportControl | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(payload) as unknown;
  } catch {
    return null;
  }
  if (!isRecord(parsed) || !isTransportControlType(parsed.type) || typeof parsed.message !== 'string') {
    return null;
  }
  const retryAfter = parsed.retry_after_ms;
  return {
    type: parsed.type,
    message: parsed.message,
    ...(typeof retryAfter === 'number' && Number.isFinite(retryAfter) && retryAfter >= 0
      ? { retry_after_ms: retryAfter }
      : {}),
  };
}

/**
 * Extract complete SSE blocks, preserving each frame's `id:` alongside its JSON
 * payload. Both CRLF and LF framing are accepted, including a CRLF split between
 * network chunks. Keepalive comments are ignored. `karucode.transport` controls
 * have no runtime envelope and never become ordinary replayable events.
 */
export function consumeSseFrames(buffer: string): { frames: SseFrame[]; rest: string } {
  const frames: SseFrame[] = [];
  let rest = buffer;
  for (;;) {
    // Looking for the boundary in the original buffer (rather than normalising
    // each network chunk) also handles the chunks ending in CR / starting in LF.
    const boundary = /\r?\n\r?\n/.exec(rest);
    if (!boundary) break;
    const block = rest.slice(0, boundary.index);
    rest = rest.slice(boundary.index + boundary[0].length);
    const data: string[] = [];
    let id: string | null = null;
    let eventName = '';
    for (const line of block.split(/\r?\n/)) {
      if (line.startsWith('data:')) data.push(line.slice(5).replace(/^ /, ''));
      if (line.startsWith('event:')) eventName = line.slice(6).replace(/^ /, '');
      if (line.startsWith('id:')) {
        const value = line.slice(3).replace(/^ /, '');
        // The SSE algorithm ignores id values containing NUL. Decimal validation
        // happens where the value is used as the server's sequence cursor.
        if (!value.includes('\0')) id = value;
      }
    }
    if (data.length === 0) continue;
    const payload = data.join('\n');
    if (eventName === 'karucode.transport') {
      const transport = parseTransportControl(payload);
      if (transport) frames.push({ id, event: null, transport });
      continue;
    }
    const event = parseEventPayload(payload);
    if (event) frames.push({ id, event });
  }
  return { frames, rest };
}

/** Extract just event payloads for data-only consumers (the original helper). */
export function consumeSseBuffer(buffer: string): { events: RawEvent[]; rest: string } {
  const consumed = consumeSseFrames(buffer);
  const events: RawEvent[] = [];
  for (const frame of consumed.frames) {
    if (frame.event) events.push(frame.event);
  }
  return { events, rest: consumed.rest };
}

/**
 * One fetch-based SSE stream (so the Authorization header travels) with
 * exponential backoff plus jitter. The newest delivered SSE id survives every
 * reconnect and is sent as `Last-Event-ID`; replayed frames are deduplicated
 * against that same persistent watermark rather than being accepted afresh on
 * each connection.
 */
export function startEventStream(options: EventStreamOptions): EventStreamHandle {
  const minDelayMs = options.minDelayMs ?? 500;
  const maxDelayMs = options.maxDelayMs ?? 15_000;
  const idleTimeoutMs = options.idleTimeoutMs ?? 45_000;
  const random = options.random ?? Math.random;
  const sleep = options.sleep ?? defaultSleep;
  const fetchImpl = options.fetchImpl ?? globalThis.fetch;
  const url = options.url ?? '/api/events';
  // A session-filtered stream drops other sessions' frames after stamping them, so
  // holes are normal there and must not be reported as loss.
  const filtered = /[?&]session=/.test(url);

  const controller = new AbortController();
  let stopped = false;
  let state: ConnectionState = 'idle';
  // Null means a fresh connection. Once an event arrives this persists for the
  // lifetime of the stream handle, including a session-filtered URL.
  let lastEventId: number | null = null;
  // A process epoch scopes the otherwise monotonically increasing sequence. This
  // deliberately survives reconnects of this handle but is never shared globally.
  let eventEpoch: string | null = null;

  const setState = (next: ConnectionState, detail?: EventStreamStatusDetail): void => {
    state = next;
    options.onStatus?.(next, detail);
  };

  const backoffDelay = (attempt: number): number => {
    const base = Math.min(maxDelayMs, minDelayMs * 2 ** attempt);
    // Half-to-full jitter avoids a thundering herd of reconnects.
    return Math.max(1, Math.round(base * (0.5 + random() * 0.5)));
  };

  /**
   * One connection attempt, including a deadline for the handshake: the idle
   * watchdog only guards the body, so a request that half-opens (sleep, captive
   * portal, dead proxy) would otherwise sit at `connecting` forever.
   */
  const connect = async (): Promise<Response> => {
    const headers: Record<string, string> = { Accept: 'text/event-stream' };
    if (options.token) headers.Authorization = `Bearer ${options.token}`;
    if (lastEventId !== null) headers['Last-Event-ID'] = String(lastEventId);
    const init: RequestInit = {
      headers,
      signal: controller.signal,
      credentials: 'same-origin',
      cache: 'no-store',
    };
    if (idleTimeoutMs <= 0) return fetchImpl(url, init);
    const attemptController = new AbortController();
    const relay = (): void => attemptController.abort();
    controller.signal.addEventListener('abort', relay, { once: true });
    let expired = false;
    const timer = setTimeout(() => {
      expired = true;
      attemptController.abort();
    }, idleTimeoutMs);
    try {
      return await fetchImpl(url, { ...init, signal: attemptController.signal });
    } catch (error) {
      if (expired) {
        throw new Error(`the event stream did not answer within ${describeMs(idleTimeoutMs)}`);
      }
      throw error;
    } finally {
      clearTimeout(timer);
      controller.signal.removeEventListener('abort', relay);
    }
  };

  /** Run a caller-supplied handler without letting it kill the stream. */
  const safely = (handler: () => void): void => {
    try {
      handler();
    } catch (error) {
      // A reducer bug must not take the reconnect loop down with it.
      console.error('karucode: event handler threw', error);
    }
  };

  const run = async (): Promise<void> => {
    let attempt = 0;
    // Persistent across connections: replay may include the cursor frame again
    // when delivery and disconnect race. Id-less servers fall back to `seq`.
    let lastSeq = Number.NEGATIVE_INFINITY;
    const sequenceFor = (frame: SseFrame): number => {
      if (!frame.event) return Number.NaN;
      const parsedId = frame.id !== null && /^\d+$/.test(frame.id) ? Number(frame.id) : Number.NaN;
      return Number.isSafeInteger(parsedId) && parsedId >= 0 ? parsedId : frame.event.seq;
    };
    const advanceCursor = (sequence: number): void => {
      if (!Number.isFinite(sequence)) return;
      lastSeq = Math.max(lastSeq, sequence);
      lastEventId = Math.max(lastEventId ?? 0, sequence);
    };
    while (!stopped && !controller.signal.aborted) {
      setState(attempt === 0 ? 'connecting' : 'reconnecting', { attempt });
      try {
        const response = await connect();

        if (response.status === 401) {
          setState('unauthorized', { attempt, error: 'Authentication required' });
          return;
        }
        if (response.status === 403) {
          // The Host allowlist refused this origin. Retrying cannot help; the user
          // has to fix KARUCODE_ALLOWED_HOSTS, so stop instead of hammering.
          setState('blocked', {
            attempt,
            error:
              'the server refused this origin (Host allowlist); add it to KARUCODE_ALLOWED_HOSTS',
          });
          return;
        }
        if (!response.ok || !response.body) {
          throw new Error(`event stream failed with HTTP ${response.status}`);
        }

        const nextEpoch = response.headers.get('X-Karucode-Event-Epoch')?.trim() || null;
        if (nextEpoch && nextEpoch !== eventEpoch) {
          // seq and Last-Event-ID are only meaningful inside one server process.
          // Reset both before reading the first new-epoch frame, so old 50 -> new 1
          // is a fresh valid stream rather than a permanently discarded suffix.
          const changedEpoch = eventEpoch !== null || lastSeq !== Number.NEGATIVE_INFINITY;
          eventEpoch = nextEpoch;
          if (changedEpoch) {
            lastSeq = Number.NEGATIVE_INFINITY;
            lastEventId = null;
            attempt = 0;
            safely(() => options.onResync?.());
          }
        }

        setState('open', { attempt });
        safely(() => options.onOpen?.());

        const watermark = (): number => (lastSeq === Number.NEGATIVE_INFINITY ? 0 : lastSeq);
        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = '';
        let closed = false;

        const deliver = (frames: SseFrame[]): void => {
          for (const frame of frames) {
            if (frame.transport) {
              const transport = frame.transport;
              // Not an event and not a cursor: even id 0 after old seq 50 must be
              // visible, while the next reconnect still asks for frames after 50.
              setState('reconnecting', { attempt, error: transport.message });
              if (TRANSPORT_CONTROL_RECOVERY[transport.type] === 'resync') {
                // Overflow proves loss even if replay makes the next live event look
                // contiguous. Keep the real cursor, repair from a snapshot, and let
                // the ordinary EOF/reconnect path continue rather than stopping it.
                attempt = 0;
                safely(() => options.onResync?.());
              }
              continue;
            }
            const event = frame.event;
            if (!event) continue;
            // Resync is a transport control frame, not a replayable event. Its
            // watermark can equal an already seen sequence, including on filtered
            // streams, but it must still trigger recovery.
            if (event.type === 'resync') {
              advanceCursor(sequenceFor(frame));
              attempt = 0;
              safely(() => options.onResync?.());
              continue;
            }
            // Lag markers are also transport controls and bypass deduplication.
            if (event.type === 'stream_lagged') {
              const skipped = typeof event.skipped === 'number' ? event.skipped : 0;
              const after = watermark();
              attempt = 0;
              // The marker is stamped with the last *dropped* seq, so it can become
              // the watermark: the next live frame is `seq + 1` and the hole is
              // reported once, here, instead of again on that frame.
              advanceCursor(sequenceFor(frame));
              if (!filtered) {
                safely(() => options.onGap?.({ after, seq: event.seq, missed: skipped }));
              }
              safely(() => options.onEvent(event));
              continue;
            }
            const sequence = sequenceFor(frame);
            if (sequence <= lastSeq) continue;
            const missed = lastSeq === Number.NEGATIVE_INFINITY ? 0 : sequence - lastSeq - 1;
            const after = watermark();
            advanceCursor(sequence);
            // A frame arrived, so this connection works: forget earlier failures
            // instead of backing off forever, but only once it has proven itself.
            attempt = 0;
            if (missed > 0 && !filtered) {
              safely(() => options.onGap?.({ after, seq: sequence, missed }));
            }
            safely(() => options.onEvent(event));
          }
        };

        try {
          while (!stopped) {
            const read = await readWithIdleTimeout(reader, idleTimeoutMs, controller.signal);
            if (read.kind === 'aborted') return;
            if (read.kind === 'idle') {
              throw new Error(
                `no event-stream data for ${describeMs(idleTimeoutMs)} (the connection went silent)`,
              );
            }
            if (read.kind === 'error') throw read.error;
            if (read.done) {
              closed = true;
              break;
            }
            buffer += decoder.decode(read.value, { stream: true });
            const consumed = consumeSseFrames(buffer);
            buffer = consumed.rest;
            deliver(consumed.frames);
          }
          if (!stopped) {
            buffer += decoder.decode();
            const tail = consumeSseFrames(buffer + '\n\n');
            deliver(tail.frames);
          }
        } finally {
          if (!closed) await cancelReader(reader);
        }

        if (stopped) return;
        // A clean close is not a failure: say nothing, so the UI does not raise an
        // error banner for a server restart or a proxy recycling the connection.
        setState('reconnecting', { attempt });
      } catch (error) {
        if (stopped || controller.signal.aborted) return;
        const message = error instanceof Error ? error.message : String(error);
        setState('reconnecting', { attempt, error: message });
      }

      const delay = backoffDelay(attempt);
      attempt += 1;
      await sleep(delay, controller.signal);
    }
  };

  void run();

  return {
    stop(): void {
      stopped = true;
      controller.abort();
      setState('idle');
    },
    getState(): ConnectionState {
      return state;
    },
  };
}