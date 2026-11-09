import {
  startEventStream,
  type EventStreamHandle,
  type EventStreamOptions,
} from './events';

/**
 * One event stream per browser, however many tabs are open.
 *
 * # Why
 *
 * Every tab used to open its own `/api/events`. That is one long-lived HTTP/1.1
 * connection *per tab*, and a browser allows six connections per origin: with
 * six tabs open the pool is entirely held by streams that never end, so every
 * other request - the health check that gates boot, the session list a project
 * switch needs - waits for a socket that is never freed. Measured with seven
 * tabs open against one server: `/api/health` went from 1.6 ms to a 30 s timeout
 * and the UI booted to "Could not reach karucode · The server did not answer
 * within 30s". Even at three or four tabs the remaining sockets are few enough
 * that a 2 MB response holds up everything behind it.
 *
 * So the first tab to open the stream keeps it, and relays every frame to its
 * siblings over a `BroadcastChannel`. Siblings have no open connection of their
 * own, which returns five of the six sockets to ordinary requests, and the
 * server encodes and writes the stream once instead of N times.
 *
 * # Who is the leader
 *
 * The Web Locks API decides, because it is the one primitive that (a) guarantees
 * a single holder across tabs, (b) queues the others so the next tab takes over
 * when the holder goes away, and (c) releases the lock when a tab is closed,
 * crashes or is discarded, with no heartbeat to get wrong. A tab that stops its
 * stream releases the lock and the next waiter starts a stream of its own.
 *
 * A follower relays everything a caller of `startEventStream` would have heard:
 * frames, connection status, the open notification, sequence gaps and resync
 * requests. Its `getState()` reports the leader's state, so the UI's connection
 * badge means the same thing in every tab.
 *
 * # What is *not* shared
 *
 * Only the network stream. Each tab keeps its own store, its own session, its own
 * terminal socket and its own resync, exactly as before.
 *
 * Falls back to a private stream when the browser has neither `BroadcastChannel`
 * nor `navigator.locks` (jsdom in tests, very old browsers), because a tab with
 * no stream at all would be worse than an extra socket.
 */

/** The channel every tab of this origin shares. */
export const EVENT_CHANNEL_NAME = 'karucode.events';
/** The Web Lock whose holder owns the one network stream. */
export const EVENT_LEADER_LOCK = 'karucode.event-stream';

type Relay =
  | { kind: 'event'; from: string; event: Parameters<EventStreamOptions['onEvent']>[0] }
  | { kind: 'status'; from: string; state: Parameters<NonNullable<EventStreamOptions['onStatus']>>[0]; detail?: Parameters<NonNullable<EventStreamOptions['onStatus']>>[1] }
  | { kind: 'open'; from: string }
  | { kind: 'gap'; from: string; gap: Parameters<NonNullable<EventStreamOptions['onGap']>>[0] }
  | { kind: 'resync'; from: string }
  | { kind: 'closing'; from: string }
  | { kind: 'hello'; from: string };

/** What the module needs from the platform. Both are absent in jsdom. */
interface Platform {
  locks?: Pick<LockManager, 'request'>;
  channel?: typeof BroadcastChannel;
}

function platform(): Platform {
  const locks =
    typeof navigator !== 'undefined' && 'locks' in navigator
      ? (navigator.locks as Pick<LockManager, 'request'> | undefined)
      : undefined;
  const channel = typeof BroadcastChannel === 'function' ? BroadcastChannel : undefined;
  return { locks, channel };
}

/**
 * Start the shared stream. The returned handle has the same contract as
 * {@link startEventStream}: `stop()` is final, and `getState()` reports the
 * connection this tab is following.
 */
export function startSharedEventStream(options: EventStreamOptions): EventStreamHandle {
  const { locks, channel: Channel } = platform();
  if (!locks?.request || !Channel) return startEventStream(options);

  const id = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
  const channel = new Channel(EVENT_CHANNEL_NAME);
  let stopped = false;
  let leader: EventStreamHandle | null = null;
  let release: (() => void) | null = null;
  // A follower reports the leader's state; a tab that has not heard anything yet
  // is connecting, which is also what its own stream would say at this point.
  let state: ReturnType<EventStreamHandle['getState']> = 'connecting';
  // Which tab this one is following. Relays are asynchronous, so a leader that
  // has already handed over can still have a `closing` in flight; without this,
  // that stale frame would flip the *new* leader's UI back to connecting.
  let leaderId: string | null = null;

  const post = (message: Relay): void => {
    if (stopped) return;
    try {
      channel.postMessage(message);
    } catch {
      // A closed channel, or a frame structured clone cannot carry. The tab
      // still has its own state; losing a relay must never break the stream.
    }
  };

  const becomeLeader = (): void => {
    // From here on this tab owns the stream: it follows nothing, so a relay still
    // in flight is ignored rather than applied over its own newer state.
    leaderId = null;
    leader = startEventStream({
      ...options,
      onEvent: (event) => {
        options.onEvent(event);
        post({ kind: 'event', from: id, event });
      },
      onStatus: (next, detail) => {
        state = next;
        options.onStatus?.(next, detail);
        post({ kind: 'status', from: id, state: next, detail });
      },
      onOpen: () => {
        options.onOpen?.();
        post({ kind: 'open', from: id });
      },
      onGap: (gap) => {
        options.onGap?.(gap);
        post({ kind: 'gap', from: id, gap });
      },
      onResync: () => {
        options.onResync?.();
        post({ kind: 'resync', from: id });
      },
    });
  };

  channel.onmessage = (event: MessageEvent<Relay>) => {
    const message = event.data;
    if (stopped || !message || typeof message !== 'object' || message.from === id) return;
    // A tab that has just opened asks the leader what the stream is doing, so a
    // follower that joins while the leader is already open can show "connected"
    // instead of sitting on its own initial state until something changes.
    if (message.kind === 'hello') {
      if (leader !== null) post({ kind: 'status', from: id, state });
      return;
    }
    // This tab owns the stream: a relay still in flight from a previous leader
    // must not be applied over its own, newer state.
    if (leader !== null) return;
    if (message.kind === 'closing' && message.from !== leaderId) return;
    leaderId = message.from;
    switch (message.kind) {
      case 'event':
        options.onEvent(message.event);
        return;
      case 'status':
        state = message.state;
        options.onStatus?.(message.state, message.detail);
        return;
      case 'open':
        options.onOpen?.();
        return;
      case 'gap':
        options.onGap?.(message.gap);
        return;
      case 'resync':
        options.onResync?.();
        return;
      case 'closing':
        // The leader we were following is going away. The lock is released right
        // after this, so this tab (or the next waiter) opens its own stream.
        leaderId = null;
        if (state === 'open') {
          state = 'connecting';
          options.onStatus?.('connecting');
        }
        return;
      default:
        return;
    }
  };

  // Announce ourselves: whoever is already leading answers with its state.
  post({ kind: 'hello', from: id });

  void locks
    .request(EVENT_LEADER_LOCK, { mode: 'exclusive' }, () => {
      if (stopped) return Promise.resolve();
      becomeLeader();
      // Holding the lock *is* this promise staying pending: resolving it hands
      // the stream to the next tab in the queue.
      return new Promise<void>((resolve) => {
        release = resolve;
      });
    })
    .catch(() => {
      // Lock acquisition can only fail for reasons outside our control (an
      // opaque origin, a revoked permission). A tab with no stream would show a
      // permanently disconnected UI, so fall back to a private one.
      if (!stopped) becomeLeader();
    });

  return {
    stop: () => {
      if (stopped) return;
      // Tell the siblings before releasing the lock: the next leader is already
      // queued and will be starting its stream on the same tick.
      post({ kind: 'closing', from: id });
      stopped = true;
      leader?.stop();
      leader = null;
      release?.();
      release = null;
      // Closing immediately can drop the message queued above.
      setTimeout(() => channel.close(), 0);
    },
    getState: () => state,
  };
}
