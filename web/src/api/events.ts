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
   * is expected to refetch the session, because the stream never replays.
   * Only meaningful for an unfiltered stream: a `?session=` filter makes holes
   * normal.
   */
  onGap?: (gap: EventStreamGap) => void;
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

/**
 * Extracts complete SSE blocks from `buffer`. Returns the parsed events and the
 * unconsumed remainder. Keepalive comments are ignored.
 */
export function consumeSseBuffer(buffer: string): { events: RawEvent[]; rest: string } {
  const events: RawEvent[] = [];
  let rest = buffer.replace(/\r\n/g, '\n');
  for (;;) {
    const boundary = rest.indexOf('\n\n');
    if (boundary === -1) break;
    const block = rest.slice(0, boundary);
    rest = rest.slice(boundary + 2);
    const data = block
      .split('\n')
      .filter((line) => line.startsWith('data:'))
      .map((line) => line.slice(5).replace(/^ /, ''))
      .join('\n');
    if (data === '') continue;
    const event = parseEventPayload(data);
    if (event) events.push(event);
  }
  return { events, rest };
}

/**
 * One fetch-based SSE stream (so the Authorization header travels) with
 * exponential backoff plus jitter. Sequence numbers are deduplicated per
 * connection: the server may restart its counter between connections, and the
 * caller refetches the open session after every reconnect.
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

        setState('open', { attempt });
        safely(() => options.onOpen?.());

        // Dedupe scope is this connection only.
        let lastSeq = Number.NEGATIVE_INFINITY;
        const watermark = (): number => (lastSeq === Number.NEGATIVE_INFINITY ? 0 : lastSeq);
        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = '';
        let closed = false;

        const deliver = (events: RawEvent[]): void => {
          for (const event of events) {
            // A lag marker describes the transport, not the session: it is stamped
            // with a seq this client may already have seen, so it is exempt from the
            // dedupe and must never advance the watermark.
            if (event.type === 'stream_lagged') {
              const skipped = typeof event.skipped === 'number' ? event.skipped : 0;
              const after = watermark();
              attempt = 0;
              // The marker is stamped with the last *dropped* seq, so it can become
              // the watermark: the next live frame is `seq + 1` and the hole is
              // reported once, here, instead of again on that frame.
              lastSeq = Math.max(lastSeq, event.seq);
              if (!filtered) {
                safely(() => options.onGap?.({ after, seq: event.seq, missed: skipped }));
              }
              safely(() => options.onEvent(event));
              continue;
            }
            if (event.seq <= lastSeq) continue;
            const missed = lastSeq === Number.NEGATIVE_INFINITY ? 0 : event.seq - lastSeq - 1;
            const after = watermark();
            lastSeq = event.seq;
            // A frame arrived, so this connection works: forget earlier failures
            // instead of backing off forever, but only once it has proven itself.
            attempt = 0;
            if (missed > 0 && !filtered) {
              safely(() => options.onGap?.({ after, seq: event.seq, missed }));
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
            const consumed = consumeSseBuffer(buffer);
            buffer = consumed.rest;
            deliver(consumed.events);
          }
          if (!stopped) {
            buffer += decoder.decode();
            const tail = consumeSseBuffer(buffer + '\n\n');
            deliver(tail.events);
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