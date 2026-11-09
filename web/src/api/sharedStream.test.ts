import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ConnectionState, RawEvent } from './types';
import {
  EVENT_CHANNEL_NAME,
  EVENT_LEADER_LOCK,
  startSharedEventStream,
} from './sharedStream';

/**
 * jsdom has neither `BroadcastChannel` nor `navigator.locks`, which is also why
 * the module falls back to a private stream there. These fakes give the tests
 * the real semantics that matter: messages reach every *other* channel of the
 * same name on a later task, and a lock has one holder with a FIFO queue.
 */
class FakeChannel {
  static registry = new Map<string, Set<FakeChannel>>();
  onmessage: ((event: MessageEvent) => void) | null = null;
  private closed = false;

  constructor(readonly name: string) {
    const peers = FakeChannel.registry.get(name) ?? new Set<FakeChannel>();
    peers.add(this);
    FakeChannel.registry.set(name, peers);
  }

  postMessage(data: unknown): void {
    if (this.closed) return;
    const peers = [...(FakeChannel.registry.get(this.name) ?? [])];
    for (const peer of peers) {
      if (peer === this || peer.closed) continue;
      setTimeout(() => peer.deliver(data), 0);
    }
  }

  private deliver(data: unknown): void {
    if (this.closed) return;
    this.onmessage?.({ data } as MessageEvent);
  }

  close(): void {
    this.closed = true;
    FakeChannel.registry.get(this.name)?.delete(this);
  }

  static reset(): void {
    FakeChannel.registry.clear();
  }
}

class FakeLocks {
  private holders = new Set<string>();
  private queue = new Map<string, Array<() => void>>();

  async request(name: string, _options: unknown, callback: () => Promise<unknown>): Promise<unknown> {
    if (this.holders.has(name)) {
      await new Promise<void>((resolve) => {
        const waiting = this.queue.get(name) ?? [];
        waiting.push(resolve);
        this.queue.set(name, waiting);
      });
    }
    this.holders.add(name);
    try {
      return await callback();
    } finally {
      this.holders.delete(name);
      const next = this.queue.get(name)?.shift();
      if (next) next();
    }
  }
}

interface OpenStream {
  response: Response;
  push: (chunk: string) => void;
  end: () => void;
}

/** A response body the test controls frame by frame. */
function openSse(): OpenStream {
  const encoder = new TextEncoder();
  let controller: ReadableStreamDefaultController<Uint8Array>;
  const body = new ReadableStream<Uint8Array>({
    start(c) {
      controller = c;
    },
  });
  return {
    response: new Response(body, {
      status: 200,
      headers: { 'content-type': 'text/event-stream' },
    }),
    push: (chunk: string) => controller.enqueue(encoder.encode(chunk)),
    end: () => controller.close(),
  };
}

function frame(seq: number, delta = 'x'): string {
  return `id: ${seq}\ndata: ${JSON.stringify({
    seq,
    ts_ms: 1_700_000_000_000 + seq,
    session_id: 's_1',
    run_id: 'r_1',
    type: 'text_delta',
    delta,
  })}\n\n`;
}

const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 5));

describe('the event stream shared between tabs', () => {
  beforeEach(() => {
    FakeChannel.reset();
    vi.stubGlobal('BroadcastChannel', FakeChannel as unknown as typeof BroadcastChannel);
    Object.defineProperty(navigator, 'locks', {
      value: new FakeLocks(),
      configurable: true,
      writable: true,
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    Reflect.deleteProperty(navigator as unknown as Record<string, unknown>, 'locks');
  });

  it('opens one network stream for the whole browser and relays its frames', async () => {
    const stream = openSse();
    const fetchImpl = vi.fn(async () => stream.response);
    const leaderFrames: RawEvent[] = [];
    const followerFrames: RawEvent[] = [];

    const leader = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: (event) => leaderFrames.push(event),
    });
    await vi.waitFor(() => expect(fetchImpl).toHaveBeenCalledTimes(1));

    const follower = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: (event) => followerFrames.push(event),
    });
    await flush();
    // The second tab never opened a connection of its own...
    expect(fetchImpl).toHaveBeenCalledTimes(1);

    stream.push(frame(1, 'hello'));
    await flush();

    // ...but both saw the frame.
    expect(leaderFrames.map((event) => event.delta)).toEqual(['hello']);
    expect(followerFrames.map((event) => event.delta)).toEqual(['hello']);
    expect(follower.getState()).toBe('open');

    leader.stop();
    follower.stop();
    stream.end();
  });

  it('hands the stream to the next tab when the leader stops', async () => {
    const first = openSse();
    const second = openSse();
    let call = 0;
    const fetchImpl = vi.fn(async () => {
      call += 1;
      return call === 1 ? first.response : second.response;
    });
    const followerFrames: RawEvent[] = [];
    const states: ConnectionState[] = [];

    const leader = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: vi.fn(),
    });
    await vi.waitFor(() => expect(fetchImpl).toHaveBeenCalledTimes(1));

    const follower = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: (event) => followerFrames.push(event),
      onStatus: (state) => states.push(state),
    });
    await flush();
    expect(fetchImpl).toHaveBeenCalledTimes(1);

    leader.stop();
    first.end();

    // The follower is now the leader: it opened its own stream.
    await vi.waitFor(() => expect(fetchImpl).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(follower.getState()).toBe('open'));

    second.push(frame(2, 'after the handover'));
    await vi.waitFor(() => expect(followerFrames.map((event) => event.delta)).toEqual(['after the handover']));
    expect(states).toContain('connecting');

    follower.stop();
    second.end();
  });

  it('relays a sequence gap so the follower refetches what it missed', async () => {
    const stream = openSse();
    const fetchImpl = vi.fn(async () => stream.response);
    const gaps: number[] = [];

    const leader = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: vi.fn(),
    });
    await vi.waitFor(() => expect(fetchImpl).toHaveBeenCalledTimes(1));

    const follower = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: vi.fn(),
      onGap: (gap) => gaps.push(gap.missed),
    });
    await flush();

    stream.push(frame(1));
    await flush();
    stream.push(frame(6));
    await vi.waitFor(() => expect(gaps.length).toBeGreaterThan(0));
    expect(gaps[0]).toBe(4);

    leader.stop();
    follower.stop();
    stream.end();
  });

  it('tells a tab that joins late what the stream is already doing', async () => {
    const stream = openSse();
    const fetchImpl = vi.fn(async () => stream.response);
    const leader = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: vi.fn(),
    });
    await vi.waitFor(() => expect(fetchImpl).toHaveBeenCalledTimes(1));
    await vi.waitFor(() => expect(leader.getState()).toBe('open'));

    // Joins while the leader is already streaming: no status event will ever be
    // broadcast for it, so it has to ask.
    const follower = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: vi.fn(),
    });
    await vi.waitFor(() => expect(follower.getState()).toBe('open'));

    leader.stop();
    follower.stop();
    stream.end();
  });

  it('keeps a private stream when the browser has no channels or locks', async () => {
    vi.unstubAllGlobals();
    Reflect.deleteProperty(navigator as unknown as Record<string, unknown>, 'locks');
    const stream = openSse();
    const fetchImpl = vi.fn(async () => stream.response);

    const one = startSharedEventStream({ fetchImpl: fetchImpl as unknown as typeof fetch, onEvent: vi.fn() });
    const two = startSharedEventStream({ fetchImpl: fetchImpl as unknown as typeof fetch, onEvent: vi.fn() });

    await vi.waitFor(() => expect(fetchImpl).toHaveBeenCalledTimes(2));
    one.stop();
    two.stop();
    stream.end();
  });

  it('names the channel and the lock it coordinates on', () => {
    expect(EVENT_CHANNEL_NAME).toBe('karucode.events');
    expect(EVENT_LEADER_LOCK).toBe('karucode.event-stream');
  });
});
