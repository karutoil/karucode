import { describe, expect, it, vi } from 'vitest';
import { consumeSseBuffer, consumeSseFrames, parseEventPayload, startEventStream } from './events';
import type { ConnectionState, RawEvent } from './types';

function sseResponse(chunks: string[], close = true): Response {
  const encoder = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
      if (close) controller.close();
    },
  });
  return new Response(body, {
    status: 200,
    headers: { 'content-type': 'text/event-stream' },
  });
}

function framedEvent(seq: number, delta: string, type = 'text_delta'): string {
  return `id: ${seq}\r\ndata: ${eventPayload(seq, delta, type)}\r\n\r\n`;
}

function eventPayload(seq: number, delta: string, type = 'text_delta'): string {
  return JSON.stringify({
    seq,
    ts_ms: 1700000000000 + seq,
    session_id: 's_1',
    run_id: 'r_1',
    type,
    delta,
  });
}

describe('consumeSseBuffer', () => {
  it('handles split chunks, keepalives and multiple events', () => {
    const first = consumeSseBuffer(`data: ${eventPayload(1, 'Hel').slice(0, -2)}`);
    expect(first.events).toHaveLength(0);
    expect(first.rest.length).toBeGreaterThan(0);

    const second = consumeSseBuffer(`${first.rest}lo"}\n\n: keepalive\n\ndata: ${eventPayload(2, 'lo')}\n\n`);
    expect(second.events.map((event) => event.delta)).toEqual(['Hello', 'lo']);
    expect(second.events.map((event) => event.seq)).toEqual([1, 2]);
    expect(second.rest).toBe('');
  });

  it('drops malformed payloads and missing envelopes', () => {
    const { events } = consumeSseBuffer(
      'data: {not json}\n\ndata: {"type":"text_delta"}\n\ndata: {"seq":4,"ts_ms":1,"type":"notice","message":"x"}\n\n',
    );
    expect(events).toHaveLength(1);
    expect(events[0]?.seq).toBe(4);
  });

  it('parses unknown event types but rejects non-object data', () => {
    expect(parseEventPayload('{"seq":1,"ts_ms":2,"type":"future_thing"}')?.type).toBe('future_thing');
    expect(parseEventPayload('"nope"')).toBeNull();
    expect(parseEventPayload('{"seq":"1","ts_ms":2,"type":"x"}')).toBeNull();
  });

  it('parses the SSE id independently of the JSON envelope and event name', () => {
    const payload = eventPayload(41, 'resumed');
    // SSE joins repeated data fields with a newline; split between JSON tokens,
    // not in the middle of a number or string.
    const splitAt = payload.indexOf(',"session_id"');
    const parsed = consumeSseFrames(
      `event: text_delta\r\nid: 41\r\ndata: ${payload.slice(0, splitAt)}\r\n` +
        `data: ${payload.slice(splitAt)}\r\n\r\n`,
    );
    expect(parsed.rest).toBe('');
    expect(parsed.frames).toHaveLength(1);
    expect(parsed.frames[0]?.id).toBe('41');
    expect(parsed.frames[0]?.event.seq).toBe(41);
    expect(parsed.frames[0]?.event.delta).toBe('resumed');
    // The existing data-only API remains useful to parser callers and tests.
    expect(consumeSseBuffer(`id: 41\ndata: ${payload}\n\n`).events[0]?.seq).toBe(41);
  });
});

describe('startEventStream', () => {
  it('reconnects after EOF with Last-Event-ID and dedupes replay across connections', async () => {
    const responses = [
      sseResponse([framedEvent(5, 'a'), framedEvent(5, 'duplicate')]),
      // A replay can race the old connection's last frame. The persistent cursor
      // drops that duplicate, then delivers the first genuinely new event.
      sseResponse([framedEvent(5, 'replay duplicate'), framedEvent(6, 'b')]),
      sseResponse([framedEvent(7, 'never')]),
    ];
    const fetchMock = vi.fn(
      async (_input: RequestInfo | URL, _init?: RequestInit) => responses.shift() ?? sseResponse([]),
    );
    const received: RawEvent[] = [];
    const states: ConnectionState[] = [];
    const delays: number[] = [];

    const handle = startEventStream({
      token: 'secret',
      fetchImpl: fetchMock as unknown as typeof fetch,
      minDelayMs: 10,
      maxDelayMs: 40,
      random: () => 0,
      sleep: async (ms) => {
        delays.push(ms);
      },
      onStatus: (state) => states.push(state),
      onEvent: (event) => {
        received.push(event);
        if (received.length === 2) handle.stop();
      },
    });

    await vi.waitFor(() => expect(received).toHaveLength(2));
    expect(received.map((event) => event.delta)).toEqual(['a', 'b']);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    const reconnect = fetchMock.mock.calls[1]?.[1] as RequestInit | undefined;
    expect((reconnect?.headers as Record<string, string>)['Last-Event-ID']).toBe('5');
    expect(states).toContain('connecting');
    expect(states.filter((state) => state === 'open')).toHaveLength(2);
    expect(states).toContain('reconnecting');
    // First retry: 10ms base, half jitter => 5ms.
    expect(delays[0]).toBe(5);
    const request = fetchMock.mock.calls[0]?.[1] as RequestInit | undefined;
    expect((request?.headers as Record<string, string>).Authorization).toBe('Bearer secret');
    expect((request?.headers as Record<string, string>).Accept).toBe('text/event-stream');
  });

  it.each(['/api/events', '/api/events?session=s_1'])(
    'persists Last-Event-ID and suppresses replay duplicates for %s',
    async (url) => {
      // The filtered case deliberately skips foreign sessions' global seqs;
      // the unfiltered case stays contiguous so only replay itself is tested.
      const filtered = url.includes('session=');
      const before = filtered ? 4 : 3;
      const after = filtered ? 7 : 4;
      const responses = [
        sseResponse([framedEvent(2, 'first'), framedEvent(before, 'before disconnect')]),
        sseResponse([framedEvent(before, 'duplicate'), framedEvent(after, 'after reconnect')]),
        sseResponse([]),
      ];
      const fetchMock = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => responses.shift() ?? sseResponse([]));
      const gaps: unknown[] = [];
      const received: RawEvent[] = [];
      const handle = startEventStream({
        url,
        fetchImpl: fetchMock as unknown as typeof fetch,
        minDelayMs: 1,
        maxDelayMs: 1,
        random: () => 0,
        sleep: async () => undefined,
        onGap: (gap) => gaps.push(gap),
        onEvent: (event) => {
          received.push(event);
          if (received.length === 3) handle.stop();
        },
      });

      await vi.waitFor(() => expect(received).toHaveLength(3));
      expect(received.map((event) => event.delta)).toEqual([
        'first',
        'before disconnect',
        'after reconnect',
      ]);
      expect(fetchMock).toHaveBeenCalledTimes(2);
      const reconnect = fetchMock.mock.calls[1]?.[1] as RequestInit | undefined;
      expect((reconnect?.headers as Record<string, string>)['Last-Event-ID']).toBe(String(before));
      // Filtered streams have ordinary holes from other sessions; the unfiltered
      // stream replays through its cursor, so neither reconnect invents a gap.
      expect(gaps).toEqual([]);
      handle.stop();
    },
  );

  it('stops on 401 instead of retrying', async () => {
    const fetchMock = vi.fn(async () =>
      new Response(JSON.stringify({ error: { code: 'unauthorized', message: 'no' } }), {
        status: 401,
        headers: { 'content-type': 'application/json' },
      }),
    );
    const states: ConnectionState[] = [];
    const onEvent = vi.fn();

    const handle = startEventStream({
      fetchImpl: fetchMock as unknown as typeof fetch,
      onEvent,
      onStatus: (state) => states.push(state),
      sleep: async () => undefined,
      random: () => 0,
    });

    await vi.waitFor(() => expect(states).toContain('unauthorized'));
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(onEvent).not.toHaveBeenCalled();
    handle.stop();
  });

  it('reconnects when the stream cannot be read and keeps trying', async () => {
    const fetchMock = vi.fn(async () => new Response(null, { status: 502 }));
    const states: ConnectionState[] = [];
    const handle = startEventStream({
      fetchImpl: fetchMock as unknown as typeof fetch,
      onEvent: () => undefined,
      onStatus: (state) => states.push(state),
      sleep: async () => {
        handle.stop();
      },
      minDelayMs: 1,
      maxDelayMs: 1,
      random: () => 0,
    });

    await vi.waitFor(() => expect(states.filter((state) => state === 'reconnecting')).toHaveLength(1));
    expect(states[0]).toBe('connecting');
    handle.stop();
  });

  it('reconnects when the connection goes silent instead of hanging forever', async () => {
    // First connection opens and then never sends a byte (half-open socket:
    // laptop asleep, proxy or server killed without a FIN).
    const silent = new Response(
      new ReadableStream<Uint8Array>({
        start() {
          // Deliberately nothing: no data, no keepalive, no close.
        },
      }),
      { status: 200 },
    );
    const responses = [silent, sseResponse([`data: ${eventPayload(9, 'recovered')}\n\n`])];
    const fetchMock = vi.fn(
      async (_input: RequestInfo | URL, _init?: RequestInit) => responses.shift() ?? sseResponse([]),
    );
    const received: RawEvent[] = [];
    const errors: (string | undefined)[] = [];

    const handle = startEventStream({
      fetchImpl: fetchMock as unknown as typeof fetch,
      idleTimeoutMs: 20,
      minDelayMs: 1,
      maxDelayMs: 1,
      random: () => 0,
      sleep: async () => undefined,
      onStatus: (state, detail) => {
        if (state === 'reconnecting') errors.push(detail?.error);
      },
      onEvent: (event) => {
        received.push(event);
        if (received.length === 1) handle.stop();
      },
    });

    await vi.waitFor(() => expect(received).toHaveLength(1));
    expect(received[0]?.delta).toBe('recovered');
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(errors.some((message) => message?.includes('went silent'))).toBe(true);
    handle.stop();
  });

  it('does not treat keepalive comments as silence', async () => {
    // Chunks come from timers, not from `start()`, so the first read can be
    // waiting on the stream while the watchdog is running.
    const body = (): ReadableStream<Uint8Array> => {
      const encoder = new TextEncoder();
      return new ReadableStream<Uint8Array>({
        start(controller) {
          let sent = 0;
          const tick = (): void => {
            if (sent < 4) {
              controller.enqueue(encoder.encode(': keepalive\n\n'));
              sent += 1;
              setTimeout(tick, 5);
              return;
            }
            // Left open on purpose: a closed body is an ordinary EOF, which
            // reconnects for a different reason than the watchdog.
            controller.enqueue(encoder.encode(`data: ${eventPayload(1, 'after keepalives')}\n\n`));
          };
          setTimeout(tick, 5);
        },
      });
    };
    const fetchMock = vi.fn(async () => new Response(body(), { status: 200 }));
    const received: RawEvent[] = [];

    const handle = startEventStream({
      fetchImpl: fetchMock as unknown as typeof fetch,
      idleTimeoutMs: 60,
      minDelayMs: 1,
      maxDelayMs: 1,
      random: () => 0,
      sleep: async () => undefined,
      onEvent: (event) => received.push(event),
    });

    await vi.waitFor(() => expect(received).toHaveLength(1));
    // The keepalives reset the watchdog: one connection, no reconnect.
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(received[0]?.delta).toBe('after keepalives');
    handle.stop();
  });

  it('reports a seq hole once per gap so the caller can resync', async () => {
    const responses = [
      sseResponse([
        `data: ${eventPayload(1, 'a')}\n\n`,
        `data: ${eventPayload(2, 'b')}\n\n`,
        `data: ${eventPayload(5, 'c')}\n\n`,
        `data: ${eventPayload(6, 'd')}\n\n`,
      ]),
      // Replay resumes immediately after the persisted cursor.
      sseResponse([`data: ${eventPayload(7, 'e')}\n\n`]),
    ];
    const fetchMock = vi.fn(
      async (_input: RequestInfo | URL, _init?: RequestInit) => responses.shift() ?? sseResponse([]),
    );
    const gaps: { after: number; seq: number; missed: number }[] = [];
    const received: RawEvent[] = [];

    const handle = startEventStream({
      fetchImpl: fetchMock as unknown as typeof fetch,
      minDelayMs: 1,
      maxDelayMs: 1,
      random: () => 0,
      sleep: async () => undefined,
      onGap: (gap) => gaps.push(gap),
      onEvent: (event) => {
        received.push(event);
        if (received.length === 5) handle.stop();
      },
    });

    await vi.waitFor(() => expect(received).toHaveLength(5));
    expect(gaps).toEqual([{ after: 2, seq: 5, missed: 2 }]);
    expect(received.map((event) => event.delta)).toEqual(['a', 'b', 'c', 'd', 'e']);
    handle.stop();
  });

  it('surfaces a mid-stream read failure instead of reporting a clean close', async () => {
    const fetchMock = vi.fn(async () =>
      new Response(
        new ReadableStream<Uint8Array>({
          start(controller) {
            controller.error(new Error('ECONNRESET'));
          },
        }),
        { status: 200 },
      ),
    );
    const errors: (string | undefined)[] = [];
    const handle = startEventStream({
      fetchImpl: fetchMock as unknown as typeof fetch,
      minDelayMs: 1,
      maxDelayMs: 1,
      random: () => 0,
      onStatus: (state, detail) => {
        if (state === 'reconnecting') errors.push(detail?.error);
      },
      onEvent: () => undefined,
      sleep: async () => {
        handle.stop();
      },
    });

    await vi.waitFor(() => expect(errors.length).toBeGreaterThan(0));
    expect(errors[0]).toContain('ECONNRESET');
    handle.stop();
  });

  it('grows the backoff while a connection keeps closing without delivering a frame', async () => {
    const fetchMock = vi.fn(async () => sseResponse([]));
    const delays: number[] = [];
    const handle = startEventStream({
      fetchImpl: fetchMock as unknown as typeof fetch,
      minDelayMs: 10,
      maxDelayMs: 1000,
      random: () => 0,
      onEvent: () => undefined,
      sleep: async (ms) => {
        delays.push(ms);
        if (delays.length === 3) handle.stop();
      },
    });

    await vi.waitFor(() => expect(delays).toHaveLength(3));
    // 10 / 20 / 40 base with half jitter: the backoff must climb, not reset.
    expect(delays).toEqual([5, 10, 20]);
    handle.stop();
  });

  it('gives up on a handshake that never answers', async () => {
    const fetchMock = vi.fn(
      (_input: RequestInfo | URL, init?: RequestInit) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener('abort', () => {
            reject(new DOMException('Aborted', 'AbortError'));
          });
        }),
    );
    const errors: (string | undefined)[] = [];
    const handle = startEventStream({
      fetchImpl: fetchMock as unknown as typeof fetch,
      idleTimeoutMs: 20,
      minDelayMs: 1,
      maxDelayMs: 1,
      random: () => 0,
      onEvent: () => undefined,
      onStatus: (state, detail) => {
        if (state === 'reconnecting') errors.push(detail?.error);
      },
      sleep: async () => {
        handle.stop();
      },
    });

    await vi.waitFor(() => expect(errors.length).toBeGreaterThan(0));
    expect(errors[0]).toContain('did not answer within 20ms');
    handle.stop();
  });

  it('accepts a lag marker and does not let it shadow the next real event', async () => {
    const marker = JSON.stringify({
      seq: 3,
      ts_ms: 1700000000003,
      session_id: 's_1',
      type: 'stream_lagged',
      skipped: 2,
    });
    const responses = [
      sseResponse([
        `data: ${eventPayload(1, 'a')}\n\n`,
        `data: ${marker}\n\n`,
        `data: ${eventPayload(4, 'b')}\n\n`,
      ]),
    ];
    const fetchMock = vi.fn(
      async (_input: RequestInfo | URL, _init?: RequestInit) => responses.shift() ?? sseResponse([]),
    );
    const received: RawEvent[] = [];
    const gaps: { after: number; seq: number; missed: number }[] = [];
    const handle = startEventStream({
      fetchImpl: fetchMock as unknown as typeof fetch,
      minDelayMs: 1,
      maxDelayMs: 1,
      random: () => 0,
      sleep: async () => undefined,
      onGap: (gap) => gaps.push(gap),
      onEvent: (event) => {
        received.push(event);
        if (received.length === 3) handle.stop();
      },
    });

    await vi.waitFor(() => expect(received).toHaveLength(3));
    expect(received.map((event) => event.type)).toEqual(['text_delta', 'stream_lagged', 'text_delta']);
    // The marker reports its own skipped count, and the real seq-4 frame after it
    // is contiguous with the marker, so it is neither dropped nor double-reported.
    expect(gaps).toEqual([{ after: 1, seq: 3, missed: 2 }]);
    expect(received[2]?.delta).toBe('b');
    handle.stop();
  });

  it.each(['/api/events', '/api/events?session=s_1'])(
    'handles a resync control even at the existing watermark on %s',
    async (url) => {
      const marker = JSON.stringify({
        seq: 5, ts_ms: 1, session_id: 's_1', type: 'resync',
        requested_id: 2, oldest_available: 4, reason: 'replay gap',
      });
      const received: RawEvent[] = [];
      const onResync = vi.fn();
      const onGap = vi.fn();
      const handle = startEventStream({
        url,
        fetchImpl: vi.fn(async () => sseResponse([
          `data: ${eventPayload(5, 'before')}\n\n`,
          `event: resync\ndata: ${marker}\n\n`,
          `data: ${eventPayload(6, 'after')}\n\n`,
        ], false)) as unknown as typeof fetch,
        onResync,
        onGap,
        onEvent: (event) => received.push(event),
      });
      await vi.waitFor(() => expect(received).toHaveLength(2));
      expect(onResync).toHaveBeenCalledTimes(1);
      expect(onGap).not.toHaveBeenCalled();
      expect(received.map((event) => event.delta)).toEqual(['before', 'after']);
      handle.stop();
    },
  );

  it('advances the reconnect cursor on resync and lag controls without losing the next event', async () => {
    for (const kind of ['resync', 'stream_lagged']) {
      const payload = JSON.stringify({
        seq: 5, ts_ms: 5, session_id: 's_1', type: kind,
        ...(kind === 'resync'
          ? { requested_id: 2, oldest_available: 4 }
          : { skipped: 2 }),
      });
      const responses = [
        sseResponse([`id: 5\nevent: ${kind}\ndata: ${payload}\n\n`]),
        sseResponse([framedEvent(5, 'duplicate control cursor'), framedEvent(6, 'next live event')]),
      ];
      const fetchMock = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => responses.shift() ?? sseResponse([]));
      const received: RawEvent[] = [];
      const onResync = vi.fn();
      const handle = startEventStream({
        fetchImpl: fetchMock as unknown as typeof fetch,
        minDelayMs: 1,
        maxDelayMs: 1,
        random: () => 0,
        sleep: async () => undefined,
        onResync,
        onEvent: (event) => {
          received.push(event);
          if (event.delta === 'next live event') handle.stop();
        },
      });
      await vi.waitFor(() => expect(received.some((event) => event.delta === 'next live event')).toBe(true));
      expect((fetchMock.mock.calls[1]?.[1]?.headers as Record<string, string>)['Last-Event-ID']).toBe('5');
      expect(received.filter((event) => event.type === 'text_delta')).toHaveLength(1);
      expect(onResync).toHaveBeenCalledTimes(kind === 'resync' ? 1 : 0);
      handle.stop();
    }
  });

  it('stops cleanly when aborted while the stream is open', async () => {
    const holder: { controller: ReadableStreamDefaultController<Uint8Array> | null } = {
      controller: null,
    };
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        holder.controller = controller;
        controller.enqueue(new TextEncoder().encode(`data: ${eventPayload(1, 'x')}\n\n`));
      },
    });
    const fetchMock = vi.fn(async () => new Response(stream, { status: 200 }));
    const onEvent = vi.fn();
    const handle = startEventStream({
      fetchImpl: fetchMock as unknown as typeof fetch,
      onEvent,
      sleep: async () => undefined,
      random: () => 0,
    });
    await vi.waitFor(() => expect(onEvent).toHaveBeenCalledTimes(1));
    handle.stop();
    expect(handle.getState()).toBe('idle');
    holder.controller?.close();
  });
});