import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ConnectionState, RawEvent } from './types';
import {
  ELECTION_TIMINGS,
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
  /** True once the reader cancelled the body, i.e. the tab stopped streaming. */
  cancelled: () => boolean;
}

/** A response body the test controls frame by frame. */
function openSse(): OpenStream {
  const encoder = new TextEncoder();
  let controller: ReadableStreamDefaultController<Uint8Array>;
  let cancelled = false;
  const body = new ReadableStream<Uint8Array>({
    start(c) {
      controller = c;
    },
    cancel() {
      cancelled = true;
    },
  });
  return {
    response: new Response(body, {
      status: 200,
      headers: { 'content-type': 'text/event-stream' },
    }),
    push: (chunk: string) => controller.enqueue(encoder.encode(chunk)),
    end: () => controller.close(),
    cancelled: () => cancelled,
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

/**
 * Advance the fake clock, which is also what delivers a channel message: the fake
 * channel posts on a timer, exactly like a real one that is never synchronous.
 */
async function settle(ms = 5): Promise<void> {
  await vi.advanceTimersByTimeAsync(ms);
}

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
    vi.stubGlobal('BroadcastChannel', undefined);
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

/**
 * Gecko has no `navigator.locks` (`'locks' in navigator` is false on a real
 * Firefox), so without the election below every tab opened its own `/api/events`
 * — the six-socket exhaustion this module exists to prevent.
 */
describe('the election when the browser has no Web Locks', () => {
  beforeEach(() => {
    FakeChannel.reset();
    vi.stubGlobal('BroadcastChannel', FakeChannel as unknown as typeof BroadcastChannel);
    Reflect.deleteProperty(navigator as unknown as Record<string, unknown>, 'locks');
    // If this ever holds, every case below would silently test the Web Locks
    // path instead of the election.
    expect('locks' in navigator).toBe(false);
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it('elects the only tab and opens one stream', async () => {
    const stream = openSse();
    const fetchImpl = vi.fn(async () => stream.response);

    const tab = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: vi.fn(),
    });
    // The claim comes first: a second tab must not find the stream already taken.
    expect(fetchImpl).not.toHaveBeenCalled();

    await settle(ELECTION_TIMINGS.claimAckMs + ELECTION_TIMINGS.claimJitterMs + 400);
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    expect(tab.getState()).toBe('open');

    tab.stop();
    stream.end();
  });

  it('makes a second tab a follower that receives the relayed frames', async () => {
    const stream = openSse();
    const fetchImpl = vi.fn(async () => stream.response);
    const followerFrames: RawEvent[] = [];

    const leader = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: vi.fn(),
    });
    await settle(ELECTION_TIMINGS.claimAckMs + ELECTION_TIMINGS.claimJitterMs + 400);
    expect(fetchImpl).toHaveBeenCalledTimes(1);

    const follower = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: (event) => followerFrames.push(event),
    });
    await settle(ELECTION_TIMINGS.claimAckMs + ELECTION_TIMINGS.claimJitterMs + 400);

    // The whole point: the second tab never opened a connection of its own...
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    // ...but reports the leader's state.
    expect(follower.getState()).toBe('open');

    stream.push(frame(1, 'relayed'));
    await settle();
    expect(followerFrames.map((event) => event.delta)).toEqual(['relayed']);

    leader.stop();
    follower.stop();
    stream.end();
  });

  it('hands the stream over when the leader closes', async () => {
    const first = openSse();
    const second = openSse();
    let call = 0;
    const fetchImpl = vi.fn(async () => {
      call += 1;
      return call === 1 ? first.response : second.response;
    });
    const frames: RawEvent[] = [];
    const states: ConnectionState[] = [];

    const leader = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: vi.fn(),
    });
    await settle(ELECTION_TIMINGS.claimAckMs + ELECTION_TIMINGS.claimJitterMs + 400);

    const follower = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: (event) => frames.push(event),
      onStatus: (state) => states.push(state),
    });
    await settle(ELECTION_TIMINGS.claimAckMs + ELECTION_TIMINGS.claimJitterMs + 400);
    expect(fetchImpl).toHaveBeenCalledTimes(1);

    leader.stop();
    // The follower claimed after its own jittered delay and took the stream.
    await settle(ELECTION_TIMINGS.claimAckMs + ELECTION_TIMINGS.claimJitterMs + 400);
    expect(fetchImpl).toHaveBeenCalledTimes(2);
    expect(follower.getState()).toBe('open');

    second.push(frame(2, 'after the handover'));
    await settle();
    expect(frames.map((event) => event.delta)).toEqual(['after the handover']);
    // It said so on the way: the badge never silently keeps saying "open".
    expect(states).toContain('connecting');

    follower.stop();
    // `first` was already closed by the leader's own `stop()`.
    second.end();
  });

  it('converges to one leader when two tabs claim at the same time', async () => {
    const stream = openSse();
    const fetchImpl = vi.fn(async () => stream.response);
    const seen: string[] = [];

    // Pin the jitter to zero for both tabs so their claims land on the same tick
    // (Math.random also names the tab, hence two different ids).
    const random = vi.spyOn(Math, 'random');
    for (const value of [0.1, 0, 0.2, 0]) random.mockReturnValueOnce(value);

    const one = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: (event) => seen.push(`one:${event.delta ?? ''}`),
    });
    const two = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: (event) => seen.push(`two:${event.delta ?? ''}`),
    });

    await settle(ELECTION_TIMINGS.claimAckMs + ELECTION_TIMINGS.claimJitterMs + 400);

    // One claim won on both tabs; the loser became a follower instead of opening
    // a second stream, and a claim/ack exchange is all it took.
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    expect(one.getState()).toBe('open');
    expect(two.getState()).toBe('open');

    stream.push(frame(1, 'both'));
    await settle();
    expect(seen.sort()).toEqual(['one:both', 'two:both']);

    one.stop();
    two.stop();
    stream.end();
  });

  it('stands down when another tab turns out to be streaming too', async () => {
    // The split-brain case: an ack lost in flight leaves two tabs that both
    // believe they lead. The heartbeat of the other one is a `claim-ack` with a
    // lower id, and the id order makes exactly one of them stand down.
    const stream = openSse();
    const fetchImpl = vi.fn(async () => stream.response);

    const tab = startSharedEventStream({
      fetchImpl: fetchImpl as unknown as typeof fetch,
      onEvent: vi.fn(),
    });
    await settle(ELECTION_TIMINGS.claimAckMs + ELECTION_TIMINGS.claimJitterMs + 400);
    expect(tab.getState()).toBe('open');
    expect(stream.cancelled()).toBe(false);

    const ghost = new FakeChannel(EVENT_CHANNEL_NAME);
    // `-` sorts before every base36 tab id, so the ghost leads the tie-break.
    ghost.postMessage({ kind: 'claim-ack', from: '-ghost', state: 'open' });
    // Long enough for the handover to finish tearing the connection down.
    await settle(1500);

    // It gave up its own connection instead of holding a second one.
    expect(stream.cancelled()).toBe(true);

    ghost.close();
    tab.stop();
  });
});
