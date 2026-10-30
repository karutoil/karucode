/**
 * The notifier: which surfaces fire, for which event.
 *
 * The interesting behaviour is all in the *routing* — what a focused tab does
 * differently from a background one, and what happens when a permission is missing —
 * so every surface is injected and every outcome is asserted.
 */

import { describe, expect, it, vi } from 'vitest';
import type { NotifyPayload } from '../api/types';
import type { CueName, PlayResult, SoundEngine } from '../lib/notifySound';
import type { NotificationEnvironment, NotificationHandle } from '../lib/notifyDesktop';
import { KIND_CUES, Notifier, cueFor, normaliseKind, type NotifierDeps } from './notifier';

const basePayload: NotifyPayload = {
  id: 'run:r_1',
  kind: 'run_finished',
  title: 'Run finished',
  body: 'All 3 files updated.',
  session_id: 's_1',
  session_title: 'Fix the login redirect',
  source: 'runtime',
  sticky: false,
};

/** `shown[i]`, asserting it exists: a missing record is the failure under test. */
function nthShown(h: Harness, index: number): Harness['desktop']['shown'][number] {
  const record = h.desktop.shown[index];
  if (!record) throw new Error(`expected at least ${index + 1} notifications, got ${h.desktop.shown.length}`);
  return record;
}

/** `toasts[i]`, asserting it exists. */
function nthToast(h: Harness, index: number) {
  const record = h.toasts[index];
  if (!record) throw new Error(`expected at least ${index + 1} toasts, got ${h.toasts.length}`);
  return record;
}

interface Harness {
  notifier: Notifier;
  cues: CueName[];
  toasts: { message: string; tone: string; sessionId?: string }[];
  unread: string[];
  focused: string[];
  desktop: {
    shown: { title: string; body: string; tag?: string; silent?: boolean; requireInteraction?: boolean; onClick?: () => void }[];
  };
  setCurrentSession: (id: string | null) => void;
  setVisible: (visible: boolean) => void;
  setSoundResult: (result: PlayResult) => void;
  setDesktopEnabled: (enabled: boolean) => void;
  setPermission: (permission: NotificationPermission) => void;
}

function harness(overrides: Partial<NotifyPayload> = {}): Harness {
  void overrides;
  const cues: CueName[] = [];
  const toasts: { message: string; tone: string; sessionId?: string }[] = [];
  const unread: string[] = [];
  const focused: string[] = [];
  const shown: Harness['desktop']['shown'] = [];
  let currentSession: string | null = 's_1';
  let visible = true;
  let soundResult: PlayResult = { played: true };
  let desktopEnabled = true;
  let permission: NotificationPermission = 'granted';

  const sounds = {
    play: async (cue: CueName): Promise<PlayResult> => {
      cues.push(cue);
      return soundResult;
    },
  } as unknown as SoundEngine;

  const ctor = function (title: string, init?: NotificationOptions): NotificationHandle {
    shown.push({
      title,
      body: init?.body ?? '',
      tag: init?.tag,
      silent: init?.silent ?? undefined,
      requireInteraction: init?.requireInteraction,
      onClick: undefined,
    });
    return { onclick: null, close: () => undefined };
  } as unknown as NotificationEnvironment['NotificationCtor'];
  Object.defineProperty(ctor, 'permission', { get: () => permission });

  const desktop: NotificationEnvironment = {
    NotificationCtor: ctor as unknown as NotificationEnvironment['NotificationCtor'],
    isSecureContext: true,
    visibility: () => (visible ? 'visible' : 'hidden'),
    isMobile: () => false,
  };

  const deps: NotifierDeps = {
    sounds,
    desktop,
    currentSessionId: () => currentSession,
    isVisible: () => visible,
    focusSession: (id) => focused.push(id),
    toast: (message, tone, sessionId) => toasts.push({ message, tone, sessionId }),
    markUnread: (id) => unread.push(id),
    desktopEnabled: () => desktopEnabled,
  };

  return {
    notifier: new Notifier(deps),
    cues,
    toasts,
    unread,
    focused,
    desktop: { shown },
    setCurrentSession: (id) => {
      currentSession = id;
    },
    setVisible: (next) => {
      visible = next;
    },
    setSoundResult: (result) => {
      soundResult = result;
    },
    setDesktopEnabled: (enabled) => {
      desktopEnabled = enabled;
    },
    setPermission: (next) => {
      permission = next;
    },
  };
}

describe('the cue each kind plays', () => {
  it('maps every kind to a distinct cue', () => {
    expect(KIND_CUES.run_finished).toBe('finished');
    expect(KIND_CUES.needs_input).toBe('attention');
    expect(KIND_CUES.error).toBe('error');
    expect(KIND_CUES.info).toBe('tick');
  });

  it('honours a payload override that names a real cue', () => {
    expect(cueFor({ ...basePayload, sound: 'error' })).toBe('error');
    expect(cueFor({ ...basePayload, sound: 'tick' })).toBe('tick');
  });

  it('falls back to the kind default for an unknown override', () => {
    // A typo in a plugin must not silence a notification.
    expect(cueFor({ ...basePayload, kind: 'needs_input', sound: 'nope' })).toBe('attention');
    expect(cueFor({ ...basePayload, kind: 'error', sound: '' })).toBe('error');
  });

  it('treats an unknown kind as info rather than dropping it', () => {
    // A newer server may learn a kind before this bundle does.
    expect(normaliseKind('brand_new')).toBe('info');
    expect(normaliseKind('run_finished')).toBe('run_finished');
    expect(cueFor({ ...basePayload, kind: 'brand_new' as never })).toBe('tick');
  });
});

describe('the four surfaces', () => {
  it('always toasts and always plays the cue', async () => {
    const h = harness();
    const outcome = await h.notifier.handle(basePayload);
    expect(outcome.toast).toBe(true);
    expect(outcome.sound).toBe('played');
    expect(h.cues).toEqual(['finished']);
    expect(h.toasts).toHaveLength(1);
    expect(nthToast(h, 0).tone).toBe('success');
    expect(nthToast(h, 0).message).toContain('Run finished');
    expect(nthToast(h, 0).message).toContain('All 3 files updated.');
  });

  it('shows a desktop notification when the tab is in the background', async () => {
    const h = harness();
    h.setVisible(false);
    const outcome = await h.notifier.handle(basePayload);
    expect(outcome.desktop).toBe('shown');
    expect(h.desktop.shown).toHaveLength(1);
    expect(nthShown(h, 0).title).toBe('Run finished');
  });

  it('still notifies about another session while the tab is in front', async () => {
    // This is the case the whole feature exists for: a run in another project
    // finishing while you are looking at something else.
    const h = harness();
    h.setVisible(true);
    h.setCurrentSession('s_other');
    const outcome = await h.notifier.handle(basePayload);
    expect(outcome.desktop).toBe('shown');
    expect(outcome.badge).toBe(true);
    expect(h.unread).toEqual(['s_1']);
  });

  it('stays quiet on the desktop for the session already on screen', async () => {
    const h = harness();
    h.setVisible(true);
    h.setCurrentSession('s_1');
    const outcome = await h.notifier.handle(basePayload);
    expect(outcome.desktop).toBe('current-session');
    expect(h.desktop.shown).toHaveLength(0);
    // The toast covered it, and there is nothing unread.
    expect(outcome.toast).toBe(true);
    expect(outcome.badge).toBe(false);
    expect(h.unread).toEqual([]);
  });

  it('badges the open session when the tab is hidden', async () => {
    // A notification you could not see still counts as unread.
    const h = harness();
    h.setVisible(false);
    h.setCurrentSession('s_1');
    const outcome = await h.notifier.handle(basePayload);
    expect(outcome.badge).toBe(true);
    expect(h.unread).toEqual(['s_1']);
  });

  it('never badges a notification with no session', async () => {
    const h = harness();
    const outcome = await h.notifier.handle({
      ...basePayload,
      session_id: '',
      source: 'some-plugin',
    });
    expect(outcome.badge).toBe(false);
    expect(h.unread).toEqual([]);
    expect(outcome.toast).toBe(true);
  });
});

describe('permissions and preferences', () => {
  it('plays the cue even when notifications are denied', async () => {
    // The requirement in one test: sound does not depend on the Notification API.
    const h = harness();
    h.setPermission('denied');
    // Hidden and on another session, so the desktop path is actually evaluated
    // rather than short-circuited by the "already on screen" rule.
    h.setVisible(false);
    h.setCurrentSession('s_other');
    const outcome = await h.notifier.handle(basePayload);
    expect(outcome.sound).toBe('played');
    expect(h.cues).toEqual(['finished']);
    expect(outcome.desktop).toBe('not-granted');
    expect(h.desktop.shown).toHaveLength(0);
    // ...and the in-app surfaces still work, so nothing is lost.
    expect(outcome.toast).toBe(true);
    expect(outcome.badge).toBe(true);
  });

  it('plays the cue when the permission was never asked for', async () => {
    const h = harness();
    h.setPermission('default');
    h.setVisible(false);
    h.setCurrentSession('s_other');
    const outcome = await h.notifier.handle(basePayload);
    expect(outcome.sound).toBe('played');
    expect(outcome.desktop).toBe('not-granted');
  });

  it('plays the cue when the browser has no notification API at all', async () => {
    const h = harness();
    const notifier = new Notifier({
      ...({
        sounds: { play: async (cue: CueName) => (h.cues.push(cue), { played: true }) },
        desktop: null,
        currentSessionId: () => 's_1',
        isVisible: () => false,
        focusSession: () => undefined,
        toast: (message: string, tone: string) => h.toasts.push({ message, tone }),
        markUnread: (id: string) => h.unread.push(id),
        desktopEnabled: () => true,
      } as unknown as NotifierDeps),
    });
    const outcome = await notifier.handle(basePayload);
    expect(outcome.sound).toBe('played');
    expect(outcome.desktop).toBe('off');
  });

  it('does not show a desktop notification when the user switched it off', async () => {
    const h = harness();
    h.setDesktopEnabled(false);
    h.setVisible(false);
    const outcome = await h.notifier.handle(basePayload);
    expect(outcome.desktop).toBe('off');
    expect(h.desktop.shown).toHaveLength(0);
  });

  it('reports a muted cue without losing the other surfaces', async () => {
    const h = harness();
    h.setSoundResult({ played: false, reason: 'muted' });
    const outcome = await h.notifier.handle(basePayload);
    expect(outcome.sound).toBe('muted');
    expect(outcome.toast).toBe(true);
  });

  it('reports a blocked cue (no gesture yet) without failing', async () => {
    const h = harness();
    h.setSoundResult({ played: false, reason: 'blocked' });
    const outcome = await h.notifier.handle(basePayload);
    expect(outcome.sound).toBe('blocked');
    expect(outcome.toast).toBe(true);
  });

  it('reports a browser without Web Audio as unsupported', async () => {
    const h = harness();
    h.setSoundResult({ played: false, reason: 'unsupported' });
    const outcome = await h.notifier.handle(basePayload);
    expect(outcome.sound).toBe('unsupported');
  });
});

describe('the desktop notification itself', () => {
  it('is silent, because the app already played a cue', async () => {
    const h = harness();
    h.setVisible(false);
    await h.notifier.handle(basePayload);
    expect(nthShown(h, 0).silent).toBe(true);
  });

  it('carries a per-kind tag so alerts replace instead of stack', async () => {
    const h = harness();
    h.setVisible(false);
    await h.notifier.handle(basePayload);
    await h.notifier.handle({ ...basePayload, id: 'run:r_2' });
    expect(nthShown(h, 0).tag).toBe('kc-run_finished');
    expect(nthShown(h, 1).tag).toBe('kc-run_finished');
  });

  it('asks to stay on screen when the payload is sticky', async () => {
    const h = harness();
    h.setVisible(false);
    await h.notifier.handle({ ...basePayload, kind: 'needs_input', sticky: true });
    expect(nthShown(h, 0).requireInteraction).toBe(true);
  });

  it('falls back to the session title when there is no body', async () => {
    const h = harness();
    h.setVisible(false);
    await h.notifier.handle({ ...basePayload, body: '' });
    expect(nthShown(h, 0).body).toBe('Fix the login redirect');
  });
});

describe('dedupe', () => {
  it('ignores a replayed frame with the same id', async () => {
    const h = harness();
    expect((await h.notifier.handle(basePayload)).toast).toBe(true);
    // A reconnect replays the ring; a second sound and a second OS notification
    // for one event is the bug this prevents.
    expect((await h.notifier.handle(basePayload)).toast).toBe(false);
    expect(h.cues).toHaveLength(1);
    expect(h.toasts).toHaveLength(1);
  });

  it('treats a different id as a new notification', async () => {
    const h = harness();
    await h.notifier.handle(basePayload);
    await h.notifier.handle({ ...basePayload, id: 'run:r_2' });
    expect(h.cues).toHaveLength(2);
  });

  it('bounds the memory it keeps for dedupe', async () => {
    const h = harness();
    for (let index = 0; index < 260; index += 1) {
      await h.notifier.handle({ ...basePayload, id: `run:r_${index}` });
    }
    // Unbounded, this set would grow for the life of a long session.
    expect(h.notifier.seenCount()).toBeLessThanOrEqual(200);
    // ...and the newest ids are still remembered.
    expect((await h.notifier.handle({ ...basePayload, id: 'run:r_259' })).toast).toBe(false);
  });
});

describe('clicking the desktop notification', () => {
  it('focuses the session it is about', async () => {
    const h = harness();
    h.setVisible(false);
    const onClick = vi.fn();
    // Drive the real show path so `onClick` is the handler the module installed.
    const shown: NotificationHandle[] = [];
    const ctor = function (_title: string, _init?: NotificationOptions): NotificationHandle {
      const handle: NotificationHandle = { onclick: null, close: () => undefined };
      shown.push(handle);
      return handle;
    } as unknown as NotificationEnvironment['NotificationCtor'];
    Object.defineProperty(ctor, 'permission', { get: () => 'granted' });
    const notifier = new Notifier({
      sounds: { play: async () => ({ played: true }) } as unknown as SoundEngine,
      desktop: {
        NotificationCtor: ctor as unknown as NotificationEnvironment['NotificationCtor'],
        isSecureContext: true,
        visibility: () => 'hidden',
        isMobile: () => false,
      },
      currentSessionId: () => null,
      isVisible: () => false,
      focusSession: onClick,
      toast: () => undefined,
      markUnread: () => undefined,
      desktopEnabled: () => true,
    });
    await notifier.handle(basePayload);
    expect(shown).toHaveLength(1);
    shown[0]?.onclick?.(new Event('click'));
    expect(onClick).toHaveBeenCalledWith('s_1');
  });
});
