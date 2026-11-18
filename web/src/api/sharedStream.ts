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
 * A follower relays everything a caller of `startEventStream` would have heard:
 * frames, connection status, the open notification, sequence gaps and resync
 * requests. Its `getState()` reports the leader's state, so the UI's connection
 * badge means the same thing in every tab.
 *
 * # Who is the leader
 *
 * The Web Locks API, where it is available: it is the one primitive that
 * (a) guarantees a single holder across tabs, (b) queues the others so the next
 * tab takes over when the holder goes away, and (c) releases the lock when a tab
 * is closed, crashes or is discarded, with no heartbeat to get wrong.
 *
 * `navigator.locks` only exists in a **secure context**. On `https://` and on
 * `http://127.0.0.1` it is there in Gecko and Blink alike; on the plain-HTTP LAN
 * address this app documents for remote access, no browser has it, and neither
 * has any browser too old to know the API. That case is what the election below
 * covers - verified in Firefox 160 against a plain-HTTP origin, where two tabs
 * held one stream.
 *
 * # The election (no Web Locks)
 *
 * A tab starts as `idle`: it posts `hello` (any leader answers) and claims after
 * a short jittered delay unless an answer arrived first.
 *
 * * `claim` — "I see no leader, so I will take the stream". The claimer waits
 *   {@link ELECTION_TIMINGS.claimAckMs} for an answer. A tab that is already
 *   leading answers every `claim`/`hello` with `claim-ack`, so a claimant yields
 *   instead of opening a second stream.
 * * `claim-ack` — "I am leading, and this is my state". Posted as the answer to
 *   a claim or a hello, and then as a heartbeat: it is also the followers'
 *   liveness signal. A claimant that hears one becomes a follower.
 * * `claim` vs `claim`: two tabs can claim on the same tick. Both compare tab
 *   ids with the same rule (the lower id leads), so exactly one of them drops its
 *   claim and waits for the winner's `claim-ack`; the winner takes the stream
 *   when its ack window closes. That is the whole of the split-brain guard: the
 *   ack, plus ids that order identically in both tabs.
 * * `closing` — a leader that stops (or is going away: `pagehide`) says so, and
 *   every follower claims after its own jittered delay.
 * * silence — a follower that hears nothing for
 *   {@link ELECTION_TIMINGS.silenceMs} may claim: that covers a leader that
 *   crashed or was discarded without a `closing`.
 *
 * # What is *not* shared
 *
 * Only the network stream. Each tab keeps its own store, its own session, its own
 * terminal socket and its own resync, exactly as before.
 *
 * Falls back to a private stream when the browser has no `BroadcastChannel` (a
 * tab with no stream at all would be worse than an extra socket).
 */

/** The channel every tab of this origin shares. */
export const EVENT_CHANNEL_NAME = 'karucode.events';
/** The Web Lock whose holder owns the one network stream. */
export const EVENT_LEADER_LOCK = 'karucode.event-stream';

/**
 * The BroadcastChannel election's timings.
 *
 * `silenceMs` is deliberately generous. Gecko throttles timers in a hidden tab
 * (roughly one tick a second, and it may delay them further), and the leader is
 * usually the tab nobody is looking at. A tighter window would hand leadership
 * away from a tab that is merely quiet, and two live streams is exactly the
 * failure this module prevents. The price of the other direction is that a
 * crashed leader is replaced after half a minute rather than instantly.
 */
export const ELECTION_TIMINGS = {
  /** How often a leader says it is still there. */
  heartbeatMs: 10_000,
  /** Silence after which a follower may claim (three missed heartbeats). */
  silenceMs: 30_000,
  /** How long a claim waits for a live leader to answer it. */
  claimAckMs: 60,
  /** Upper bound of the random delay before a claim, so claims do not collide. */
  claimJitterMs: 120,
} as const;

/** The connection state a tab is reporting, from the stream's own type. */
type StreamState = ReturnType<EventStreamHandle['getState']>;
type StatusDetail = Parameters<NonNullable<EventStreamOptions['onStatus']>>[1];

type Relay =
  | { kind: 'event'; from: string; event: Parameters<EventStreamOptions['onEvent']>[0] }
  | { kind: 'status'; from: string; state: StreamState; detail?: StatusDetail }
  | { kind: 'open'; from: string }
  | { kind: 'gap'; from: string; gap: Parameters<NonNullable<EventStreamOptions['onGap']>>[0] }
  | { kind: 'resync'; from: string }
  | { kind: 'closing'; from: string }
  | { kind: 'hello'; from: string }
  | { kind: 'claim'; from: string }
  | { kind: 'claim-ack'; from: string; state: StreamState };

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
 * Which of two simultaneous claims leads: the lower tab id.
 *
 * Ids are unique, and both tabs compare the same pair the same way, so exactly
 * one of them yields. Which one wins does not matter; that it is the same one in
 * both tabs does.
 */
function claimsFirst(a: string, b: string): boolean {
  return a < b;
}

/**
 * Start the shared stream. The returned handle has the same contract as
 * {@link startEventStream}: `stop()` is final, and `getState()` reports the
 * connection this tab is following.
 */
export function startSharedEventStream(options: EventStreamOptions): EventStreamHandle {
  const { locks, channel: Channel } = platform();
  // Nothing to coordinate over: a private stream beats a tab that never connects.
  if (!Channel) return startEventStream(options);

  // Web Locks is the better primitive where it exists; the channel election is
  // only for browsers that do not have it (Gecko).
  const elected = !locks?.request;

  const id = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
  const channel = new Channel(EVENT_CHANNEL_NAME);
  let stopped = false;
  let leader: EventStreamHandle | null = null;
  let release: (() => void) | null = null;
  /** Where this tab is in the election. Only meaningful while `elected`. */
  let role: 'idle' | 'claiming' | 'leader' | 'follower' = 'idle';
  // A follower reports the leader's state; a tab that has not heard anything yet
  // is connecting, which is also what its own stream would say at this point.
  let state: StreamState = 'connecting';
  // Which tab this one is following. Relays are asynchronous, so a leader that
  // has already handed over can still have a `closing` in flight; without this,
  // that stale frame would flip the *new* leader's UI back to connecting.
  let leaderId: string | null = null;
  let claimTimer: ReturnType<typeof setTimeout> | null = null;
  let ackTimer: ReturnType<typeof setTimeout> | null = null;
  let silenceTimer: ReturnType<typeof setTimeout> | null = null;
  let heartbeatTimer: ReturnType<typeof setInterval> | null = null;

  const post = (message: Relay): void => {
    if (stopped) return;
    try {
      channel.postMessage(message);
    } catch {
      // A closed channel, or a frame structured clone cannot carry. The tab
      // still has its own state; losing a relay must never break the stream.
    }
  };

  const clear = <T extends ReturnType<typeof setTimeout> | ReturnType<typeof setInterval> | null>(
    handle: T,
  ): null => {
    if (handle !== null) clearTimeout(handle as ReturnType<typeof setTimeout>);
    return null;
  };

  const setState = (next: StreamState, detail?: StatusDetail): void => {
    state = next;
    options.onStatus?.(next, detail);
  };

  /** Forget any claim or heartbeat this tab scheduled. */
  const dropElectionTimers = (): void => {
    claimTimer = clear(claimTimer);
    ackTimer = clear(ackTimer);
    silenceTimer = clear(silenceTimer);
    heartbeatTimer = clear(heartbeatTimer);
  };

  /**
   * (Re)start the watchdog that says how long the leader may stay quiet before
   * this tab may claim. Every message from the leader resets it.
   */
  const watchLeader = (): void => {
    if (!elected) return;
    silenceTimer = clear(silenceTimer);
    silenceTimer = setTimeout(() => {
      silenceTimer = null;
      if (stopped) return;
      // No `closing` arrived: the leader crashed, was discarded, or is frozen.
      role = 'idle';
      leaderId = null;
      scheduleClaim(jitter());
    }, ELECTION_TIMINGS.silenceMs);
  };

  const jitter = (): number => Math.random() * ELECTION_TIMINGS.claimJitterMs;

  const scheduleClaim = (delay: number): void => {
    if (!elected || stopped) return;
    claimTimer = clear(claimTimer);
    claimTimer = setTimeout(attemptClaim, Math.max(0, delay));
  };

  /**
   * Announce that this tab sees no leader, then wait out the ack window. A live
   * leader answers `claim` with `claim-ack` inside it, and the answer turns this
   * tab into a follower; silence means the stream is free.
   */
  const attemptClaim = (): void => {
    claimTimer = null;
    if (!elected || stopped || leader !== null) return;
    role = 'claiming';
    post({ kind: 'claim', from: id });
    ackTimer = clear(ackTimer);
    ackTimer = setTimeout(() => {
      ackTimer = null;
      if (stopped || role !== 'claiming') return;
      becomeLeader();
    }, ELECTION_TIMINGS.claimAckMs);
  };

  const becomeLeader = (): void => {
    if (stopped || leader !== null) return;
    dropElectionTimers();
    role = 'leader';
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
    if (!elected) return;
    // Announce immediately, then keep announcing: the heartbeat is what stops a
    // follower from concluding the leader is gone.
    const announce = (): void => {
      if (!stopped && leader !== null) post({ kind: 'claim-ack', from: id, state });
    };
    announce();
    heartbeatTimer = setInterval(announce, ELECTION_TIMINGS.heartbeatMs);
  };

  /** Follow the tab that answered, and adopt the state it reported. */
  const follow = (from: string, next: StreamState, detail?: StatusDetail): void => {
    claimTimer = clear(claimTimer);
    ackTimer = clear(ackTimer);
    role = 'follower';
    leaderId = from;
    watchLeader();
    // An ack is also the heartbeat, so most of these repeat a state this tab
    // already has. Reporting the repeat would rewrite the caller's connection
    // signal — and re-render the badge — for nothing.
    if (next !== state || detail?.error) setState(next, detail);
  };

  /**
   * Stand down in favour of a tab that is streaming too.
   *
   * Only a lost `claim-ack` gets here: normally one tab leads and the rest
   * follow. Standing down costs one handover; two live streams cost the sockets
   * this module exists to protect.
   */
  const yieldLeadership = (to: string): void => {
    post({ kind: 'closing', from: id });
    leader?.stop();
    leader = null;
    heartbeatTimer = clear(heartbeatTimer);
    role = 'follower';
    leaderId = to;
    watchLeader();
    // Ask the other leader for its state rather than guessing it.
    post({ kind: 'hello', from: id });
  };

  /** A leader that goes away must hand over: `pagehide` freezes its timers. */
  const onPageHide = (): void => {
    if (stopped || leader === null) return;
    post({ kind: 'closing', from: id });
    leader.stop();
    leader = null;
    role = 'idle';
    leaderId = null;
    dropElectionTimers();
    // A page restored from the back/forward cache runs this timer on restore and
    // rejoins the election; a page that is really gone never runs it.
    scheduleClaim(jitter());
  };

  channel.onmessage = (event: MessageEvent<Relay>) => {
    const message = event.data;
    if (stopped || !message || typeof message !== 'object' || message.from === id) return;

    // This tab owns the stream: it answers questions and claims, and ignores
    // everything else. A relay still in flight from a previous leader must not be
    // applied over its own, newer state.
    if (leader !== null) {
      if (message.kind === 'hello' || message.kind === 'claim') {
        post({ kind: 'claim-ack', from: id, state });
        return;
      }
      // Two tabs can both believe they lead — an ack lost in flight is enough for
      // that, since a claimer only leads when nothing answers it. The same id
      // order that breaks a claim tie settles it here too: the tab that compares
      // second stands down, so exactly one of them keeps streaming.
      if (elected && message.kind === 'claim-ack' && claimsFirst(message.from, id)) {
        yieldLeadership(message.from);
      }
      return;
    }

    switch (message.kind) {
      case 'hello':
        // Already asked; the answer (if any) arrives as `claim-ack`.
        return;
      case 'claim':
        if (!elected) return;
        if (role === 'claiming') {
          if (!claimsFirst(message.from, id)) return;
          // The other claim wins the tie. Wait for its ack; the watchdog covers
          // it dying before it leads.
          ackTimer = clear(ackTimer);
          role = 'follower';
          leaderId = message.from;
          watchLeader();
          return;
        }
        // Somebody else is claiming: drop our own pending claim and wait for the
        // ack, which is what stops two tabs storming the channel.
        claimTimer = clear(claimTimer);
        leaderId = message.from;
        watchLeader();
        return;
      case 'claim-ack':
        // A live leader. Take its state, so the badge means the same thing here.
        follow(message.from, message.state);
        return;
      case 'closing':
        if (message.from !== leaderId) return;
        // The leader we were following is going away. Every follower claims after
        // its own jittered delay; the claim/claim-ack pair picks one.
        leaderId = null;
        role = 'idle';
        silenceTimer = clear(silenceTimer);
        if (state === 'open') setState('connecting');
        scheduleClaim(jitter());
        return;
      default:
        break;
    }

    // A relay from the leader this tab follows. A claim still waiting for its ack
    // window yields to it: the leader is demonstrably alive.
    if (role === 'claiming') ackTimer = clear(ackTimer);
    role = 'follower';
    leaderId = message.from;
    watchLeader();
    switch (message.kind) {
      case 'event':
        options.onEvent(message.event);
        return;
      case 'status':
        setState(message.state, message.detail);
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
      default:
        return;
    }
  };

  // Announce ourselves: whoever is already leading answers with its state.
  post({ kind: 'hello', from: id });

  if (elected) {
    // No lock to queue on, so this tab claims unless somebody answered first. The
    // jitter is what keeps a page that opens several tabs at once from having all
    // of them claim on the same millisecond.
    scheduleClaim(ELECTION_TIMINGS.claimAckMs + jitter());
    if (typeof window !== 'undefined') window.addEventListener('pagehide', onPageHide);
  } else {
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
  }

  return {
    stop: () => {
      if (stopped) return;
      // Tell the siblings before releasing the lock: the next leader is already
      // queued (or, without locks, waiting out its jitter) and will be starting
      // its stream on the same tick.
      post({ kind: 'closing', from: id });
      stopped = true;
      dropElectionTimers();
      if (elected && typeof window !== 'undefined') {
        window.removeEventListener('pagehide', onPageHide);
      }
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
