import { describe, expect, it, vi } from 'vitest';
import { consumeSseBuffer, parseEventPayload, startEventStream } from './events';
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
});

describe('startEventStream', () => {
  it('reconnects after EOF with backoff and dedupes seq per connection', async () => {
    const responses = [
      sseResponse([`data: ${eventPayload(5, 'a')}\n\n`, `data: ${eventPayload(5, 'duplicate')}\n\n`]),
      sseResponse([`data: ${eventPayload(5, 'b')}\n\n`]),
      sseResponse([`data: ${eventPayload(6, 'never')}\n\n`]),
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
    expect(states).toContain('connecting');
    expect(states.filter((state) => state === 'open')).toHaveLength(2);
    expect(states).toContain('reconnecting');
    // First retry: 10ms base, half jitter => 5ms.
    expect(delays[0]).toBe(5);
    const request = fetchMock.mock.calls[0]?.[1] as RequestInit | undefined;
    expect((request?.headers as Record<string, string>).Authorization).toBe('Bearer secret');
    expect((request?.headers as Record<string, string>).Accept).toBe('text/event-stream');
  });

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
    expect(handle.getState()).toBe('open');
    holder.controller?.close();
  });
});