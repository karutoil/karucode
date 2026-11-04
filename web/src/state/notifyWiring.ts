/**
 * The page's single notifier, and the gesture that makes sound possible.
 *
 * Two things live here that must not be duplicated:
 *
 * * **one `SoundEngine`** — a second AudioContext would double every cue and is a
 *   wasted resource;
 * * **one gesture listener** — the first pointer/key event after load resumes the
 *   audio context. Attached once, removed when it has done its job.
 *
 * The notifier is created lazily so a unit test that imports `state/actions` does
 * not build an AudioContext (jsdom has none) or read `Notification`.
 */

import { browserNotificationEnvironment } from '../lib/notifyDesktop';
import { SoundEngine, browserSoundEnvironment, type CueName } from '../lib/notifySound';
import type { NotifyPayload } from '../api/types';
import { Notifier, type DispatchOutcome } from './notifier';
import * as actions from './actions';
import * as store from './store';

let engine: SoundEngine | null = null;
let notifier: Notifier | null = null;
/** Set once a cue was refused for lack of a gesture, so the hint is shown once. */
let hintedGesture = false;
let gestureAttached = false;

/** The page's sound engine, configured from the persisted preferences. */
export function soundEngine(): SoundEngine {
  if (!engine) {
    engine = new SoundEngine(browserSoundEnvironment(), {
      muted: !store.soundEnabled.value,
      volume: store.soundVolume.value,
    });
  }
  return engine;
}

/** Keep the engine's mute/volume in step with the settings signals. */
export function syncSoundPreferences(): void {
  const instance = soundEngine();
  instance.setMuted(!store.soundEnabled.value);
  instance.setVolume(store.soundVolume.value);
}

/** The page's notifier. Built once. */
export function notifierInstance(): Notifier {
  if (!notifier) {
    notifier = new Notifier({
      sounds: soundEngine(),
      // The desktop capability is read through a getter so switching the
      // preference, or a permission granted in another tab, takes effect without
      // rebuilding the notifier.
      get desktop() {
        return browserNotificationEnvironment();
      },
      currentSessionId: () => store.currentSessionId.value,
      // Focus is read at emit time, never cached: a tab that was hidden when the
      // event was subscribed must still notify when the run ends.
      isVisible: () =>
        typeof document === 'undefined' ? true : document.visibilityState === 'visible',
      focusSession: (sessionId) => {
        actions.focusSession(sessionId);
      },
      toast: (message, tone, sessionId) => {
        store.pushToast(message, tone, 9000, sessionId);
      },
      // Every notification goes into the centre; the unread badge, the per-project
      // count and the title counter are all derived from that one list.
      record: (payload, read) => store.recordNotification(payload, read),
      desktopEnabled: () => store.desktopEnabled.value,
    } as ConstructorParameters<typeof Notifier>[0]);
  }
  return notifier;
}

/**
 * Handle one `notification` event from the server.
 *
 * Called from the event dispatch path, not from a component, so it runs whether or
 * not the notification's session is open.
 */
export async function handleNotification(payload: NotifyPayload): Promise<DispatchOutcome> {
  const outcome = await notifierInstance().handle(payload);
  // A blocked cue means no gesture has happened yet. Say so once: a silence that
  // looks like a bug is the failure mode this whole module tries to avoid.
  if (outcome.sound === 'blocked' && !hintedGesture) {
    hintedGesture = true;
    store.pushToast(
      'Click anywhere to enable notification sounds (the browser needs one interaction first).',
      'info',
      9000,
    );
  }
  return outcome;
}

/**
 * Resume audio on the first interaction, then stop listening.
 *
 * Browsers block playback in a tab nobody has touched. A workspace is always
 * touched eventually, so one gesture is enough for the rest of the session.
 */
export function armSoundOnFirstGesture(): () => void {
  if (typeof window === 'undefined' || gestureAttached) return () => undefined;
  gestureAttached = true;
  const arm = (): void => {
    void soundEngine().arm();
    cleanup();
  };
  const cleanup = (): void => {
    window.removeEventListener('pointerdown', arm, true);
    window.removeEventListener('keydown', arm, true);
    gestureAttached = false;
  };
  // Capture phase so a handler that stops propagation cannot prevent this.
  window.addEventListener('pointerdown', arm, true);
  window.addEventListener('keydown', arm, true);
  return cleanup;
}

/** Preview one cue from the settings screen. */
export async function previewCue(cue: CueName): Promise<DispatchOutcome['sound']> {
  const instance = soundEngine();
  // A preview is an explicit request, so a mute preference must not silence it —
  // otherwise the button cannot answer "does this work".
  const wasMuted = instance.isMuted();
  instance.setMuted(false);
  const result = await instance.play(cue);
  instance.setMuted(wasMuted);
  return result.played ? 'played' : ((result.reason ?? 'off') as DispatchOutcome['sound']);
}

/** Test seam: drop the singletons so a suite starts clean. */
export function resetNotificationSingletons(): void {
  engine = null;
  notifier = null;
  hintedGesture = false;
  gestureAttached = false;
}
