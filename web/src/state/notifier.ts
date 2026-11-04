/**
 * One reducer, five surfaces.
 *
 * A `notification` event from the server feeds every way a human can be told that
 * something happened, and this module is the only place that decides which of them
 * fires:
 *
 * | | surface | when |
 * | --- | --- | --- |
 * | 1 | in-app toast | always (the most reliable channel) |
 * | 2 | sound cue | unless muted, and unless a cue is already playing |
 * | 3 | desktop notification | permission granted, and the event is not about the open, visible session |
 * | 4 | unread badge + title counter | unless the event is about the open session |
 * | 5 | the notification centre's log | always; already-read when it was on screen |
 *
 * Keeping them in one function is what makes "off" mean off: there is no second
 * path that could still make a noise. The centre is the exception to "off" on
 * purpose: it is a history of what the server reported, and a notification nobody
 * was shown is exactly the one worth finding there later.
 *
 * ## Why the split between the sound and the desktop notification
 *
 * They are independent on purpose. `Notification.permission` says nothing about
 * whether an AudioContext may play, so a user who denies the permission still gets
 * an audible cue — which is the whole requirement. The sound is played *and* the
 * desktop notification is made silent, so one event never makes two noises.
 *
 * ## Routing
 *
 * The server notifies about every terminal run, including ones in sessions this tab
 * is not showing. That is the point: a run in another project is exactly the case
 * you would otherwise miss. So the client only suppresses the *desktop* surface and
 * the *badge* when the event is about the session in front of you, and it reads
 * focus at emit time rather than at subscribe time.
 */

import type { NotifyKind, NotifyPayload } from '../api/types';
import type { CueName, SoundEngine } from '../lib/notifySound';
import {
  showDesktopNotification,
  type NotificationEnvironment,
} from '../lib/notifyDesktop';

/** The cue each kind plays. A payload may override it with `sound`. */
export const KIND_CUES: Record<NotifyKind, CueName> = {
  run_finished: 'finished',
  needs_input: 'attention',
  error: 'error',
  info: 'tick',
};

/** Toast tone per kind: an error reads as an error, everything else as news. */
export const KIND_TONES: Record<NotifyKind, 'info' | 'success' | 'error'> = {
  run_finished: 'success',
  needs_input: 'info',
  error: 'error',
  info: 'info',
};

/**
 * Resolve the cue for a payload.
 *
 * An unknown `sound` name falls back to the kind's default rather than going
 * silent: a typo in a plugin should not make a notification invisible, and the same
 * rule is documented for `sticky`.
 */
export function cueFor(payload: NotifyPayload): CueName {
  const requested = payload.sound;
  if (requested) {
    // The payload's override must name a real cue; anything else is ignored.
    const known: CueName[] = ['finished', 'attention', 'error', 'tick'];
    if ((known as string[]).includes(requested)) return requested as CueName;
  }
  return KIND_CUES[normaliseKind(payload.kind)] ?? 'tick';
}

/** Coerce an unknown kind to `info`, so a newer server does not lose the message. */
export function normaliseKind(kind: string): NotifyKind {
  if (
    kind === 'run_finished' ||
    kind === 'needs_input' ||
    kind === 'error' ||
    kind === 'info'
  ) {
    return kind;
  }
  return 'info';
}

/** What the notifier is allowed to touch, injected for tests. */
export interface NotifierDeps {
  sounds: SoundEngine;
  /** Desktop capability; `null` keeps the whole surface off (the default until wired). */
  desktop: NotificationEnvironment | null;
  /** The session currently open in this tab, or `null`. */
  currentSessionId: () => string | null;
  /** Whether this tab is showing something (read at emit time, never cached). */
  isVisible: () => boolean;
  /** Bring the window forward and open a session. */
  focusSession: (sessionId: string) => void;
  /** An in-app toast. */
  toast: (message: string, tone: 'info' | 'success' | 'error', sessionId?: string) => void;
  /**
   * Record the notification for the notification centre.
   *
   * `read` is true when it arrived about something already in front of the reader:
   * the toast was seen, so it must not also raise an unread badge. The entry is kept
   * either way — the centre is a history, not a dismissal queue — and the per-session
   * badge, the per-project count and the title counter are all derived from it.
   */
  record: (payload: NotifyPayload, read: boolean) => void;
  /** Whether the desktop surface is switched on by the user (a browser preference). */
  desktopEnabled: () => boolean;
}

/** Why a surface did not fire, for the tests and for the settings diagnostics. */
export interface DispatchOutcome {
  toast: boolean;
  sound: 'played' | 'muted' | 'coalesced' | 'blocked' | 'unsupported' | 'off';
  desktop: 'shown' | 'off' | 'not-granted' | 'current-session' | 'visible' | 'limited';
  badge: boolean;
}

/**
 * The set of notification ids already handled.
 *
 * A reconnect replays frames from the ring, and two tabs can receive the same
 * event, so the id is the dedupe key. Bounded because a long session would
 * otherwise grow the set forever; the ids are only useful for the last few.
 */
const SEEN_LIMIT = 200;

export class Notifier {
  private readonly deps: NotifierDeps;
  private seen = new Set<string>();
  private order: string[] = [];

  constructor(deps: NotifierDeps) {
    this.deps = deps;
  }

  /** Everything one event does. Pure with respect to state, so it is testable. */
  async handle(payload: NotifyPayload): Promise<DispatchOutcome> {
    // Dedupe first: a replayed frame must not produce a second sound or a second
    // desktop notification.
    if (this.remember(payload.id)) {
      return { toast: false, sound: 'coalesced', desktop: 'off', badge: false };
    }

    const kind = normaliseKind(payload.kind);
    const sessionId = payload.session_id;
    const current = this.deps.currentSessionId();
    const aboutCurrentSession = sessionId !== '' && sessionId === current;
    const visible = this.deps.isVisible();

    // 1. The toast. Always, if the server sent one at all: it is in-app, costs
    //    nothing, and is the only surface that cannot be blocked by a permission.
    const message = payload.body ? `${payload.title} — ${payload.body}` : payload.title;
    this.deps.toast(message, KIND_TONES[kind], sessionId || undefined);

    // 2. The sound. Independent of every permission.
    const sound = await this.playCue(cueFor(payload));

    // 3. The desktop notification. Suppressed when the event is about what is
    //    already on screen: the toast and the badge cover that case.
    const desktopSkipped: DispatchOutcome['desktop'] = !this.deps.desktopEnabled()
      ? 'off'
      : aboutCurrentSession && visible
        ? 'current-session'
        : !this.deps.desktop
          ? 'off'
          : 'shown';
    const desktop = this.showDesktop(payload, kind, desktopSkipped);

    // 4. The badge, for a session the user is not looking at.
    const badge = sessionId !== '' && !(aboutCurrentSession && visible);
    // Everything is kept for the notification centre; only the badge (and the
    // per-project count derived from it) depends on whether it was already on
    // screen.
    this.deps.record(payload, !badge);

    return { toast: true, sound, desktop, badge };
  }

  /** How many ids are remembered; exposed for the bound test. */
  seenCount(): number {
    return this.seen.size;
  }

  private async playCue(cue: CueName): Promise<DispatchOutcome['sound']> {
    const result = await this.deps.sounds.play(cue);
    if (result.played) return 'played';
    // `reason` is a subset of the outcome union by construction.
    return (result.reason ?? 'off') as DispatchOutcome['sound'];
  }

  private showDesktop(
    payload: NotifyPayload,
    kind: NotifyKind,
    skip: DispatchOutcome['desktop'],
  ): DispatchOutcome['desktop'] {
    if (skip !== 'shown') return skip;
    const env = this.deps.desktop;
    if (!env) return 'off';
    const result = showDesktopNotification(env, {
      title: payload.title,
      body: payload.body || payload.session_title,
      // One tag per kind: five approvals replace each other instead of stacking.
      tag: `kc-${kind}`,
      // A sticky notification must not vanish before it is read; that is what
      // `sticky` means on the wire.
      requireInteraction: payload.sticky,
      // The app already played a cue, so the OS must stay quiet.
      silent: true,
      onClick: () => {
        if (payload.session_id) this.deps.focusSession(payload.session_id);
      },
    });
    if (result.shown) return 'shown';
    if (result.reason === 'constructor-threw') return 'limited';
    if (result.reason === 'granted' || result.reason === 'limited') return 'shown';
    return 'not-granted';
  }

  /** Record an id; `true` when it was already known. */
  private remember(id: string): boolean {
    if (this.seen.has(id)) return true;
    this.seen.add(id);
    this.order.push(id);
    if (this.order.length > SEEN_LIMIT) {
      const oldest = this.order.shift();
      if (oldest) this.seen.delete(oldest);
    }
    return false;
  }
}
