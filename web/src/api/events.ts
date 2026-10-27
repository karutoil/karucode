import type { ConnectionState, RawEvent } from './types';

export interface EventStreamStatusDetail {
  attempt: number;
  error?: string;
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
  minDelayMs?: number;
  maxDelayMs?: number;
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
  const random = options.random ?? Math.random;
  const sleep = options.sleep ?? defaultSleep;
  const fetchImpl = options.fetchImpl ?? globalThis.fetch;
  const url = options.url ?? '/api/events';

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

  const run = async (): Promise<void> => {
    let attempt = 0;
    while (!stopped && !controller.signal.aborted) {
      setState(attempt === 0 ? 'connecting' : 'reconnecting', { attempt });
      try {
        const headers: Record<string, string> = { Accept: 'text/event-stream' };
        if (options.token) headers.Authorization = `Bearer ${options.token}`;
        const response = await fetchImpl(url, {
          headers,
          signal: controller.signal,
          credentials: 'same-origin',
          cache: 'no-store',
        });

        if (response.status === 401) {
          setState('unauthorized', { attempt, error: 'Authentication required' });
          return;
        }
        if (!response.ok || !response.body) {
          throw new Error(`event stream failed with HTTP ${response.status}`);
        }

        attempt = 0;
        setState('open', { attempt });
        options.onOpen?.();

        // Dedupe scope is this connection only.
        let lastSeq = Number.NEGATIVE_INFINITY;
        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = '';
        let closed = false;
        try {
          while (!stopped) {
            const { value, done } = await reader.read();
            if (done) {
              closed = true;
              break;
            }
            buffer += decoder.decode(value, { stream: true });
            const consumed = consumeSseBuffer(buffer);
            buffer = consumed.rest;
            for (const event of consumed.events) {
              if (event.seq <= lastSeq) continue;
              lastSeq = event.seq;
              options.onEvent(event);
            }
          }
          if (!stopped) {
            buffer += decoder.decode();
            const tail = consumeSseBuffer(buffer + '\n\n');
            for (const event of tail.events) {
              if (event.seq <= lastSeq) continue;
              lastSeq = event.seq;
              options.onEvent(event);
            }
          }
        } finally {
          if (!closed) {
            try {
              await reader.cancel();
            } catch {
              // The connection is already gone; nothing to clean up.
            }
          }
        }

        if (stopped) return;
        setState('reconnecting', { attempt: 0, error: 'stream closed' });
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
    },
    getState(): ConnectionState {
      return state;
    },
  };
}