/**
 * Desktop notifications: capability detection, permission and the show path.
 *
 * Nothing here touches the real `Notification` API — jsdom has none — so the module
 * takes its constructor and its environment from the outside. What the tests pin is
 * the *decision* in each situation, especially the two that are easy to get wrong:
 * never asking for permission on load, and reporting "cannot use this here" instead
 * of failing silently.
 */

import { describe, expect, it, vi } from 'vitest';
import {
  browserNotificationEnvironment,
  desktopState,
  requestDesktopPermission,
  showDesktopNotification,
  type NotificationCtor,
  type NotificationEnvironment,
  type NotificationHandle,
} from './notifyDesktop';

interface ShownNotification {
  title: string;
  options: NotificationOptions;
  closed: boolean;
  handle: NotificationHandle;
}

function fakeBrowser(options: {
  permission?: NotificationPermission;
  secure?: boolean;
  mobile?: boolean;
  throwOnConstruct?: boolean;
  noApi?: boolean;
} = {}): {
  env: NotificationEnvironment;
  shown: ShownNotification[];
  requests: number;
  setPermission: (permission: NotificationPermission) => void;
  clickFirst: () => void;
} {
  let permission = options.permission ?? 'default';
  const shown: ShownNotification[] = [];
  let requests = 0;

  const ctor = function (
    this: unknown,
    title: string,
    init?: NotificationOptions,
  ): NotificationHandle {
    if (options.throwOnConstruct) {
      // The documented mobile behaviour: the constructor is not available and a
      // service worker would be needed instead.
      throw new TypeError('Illegal constructor');
    }
    const handle: NotificationHandle = {
      onclick: null,
      close: () => {
        const record = shown[shown.length - 1];
        if (record) record.closed = true;
      },
    };
    shown.push({ title, options: init ?? {}, closed: false, handle });
    return handle;
  } as unknown as NotificationCtor;

  Object.defineProperty(ctor, 'permission', { get: () => permission });
  (ctor as unknown as { requestPermission: () => Promise<NotificationPermission> }).requestPermission =
    () => {
      requests += 1;
      permission = 'granted';
      return Promise.resolve('granted');
    };

  const env: NotificationEnvironment = {
    NotificationCtor: options.noApi ? undefined : ctor,
    isSecureContext: options.secure ?? true,
    visibility: () => 'hidden',
    isMobile: () => options.mobile ?? false,
  };

  return {
    env,
    shown,
    get requests() {
      return requests;
    },
    setPermission: (next) => {
      permission = next;
    },
    clickFirst: () => {
      const first = shown[0];
      first?.handle.onclick?.(new Event('click'));
    },
  };
}

const note = {
  title: 'Run finished',
  body: 'All 3 files updated.',
  tag: 'run_finished',
  requireInteraction: false,
  silent: true,
};

describe('capability detection', () => {
  it('says the browser has no API rather than pretending it worked', () => {
    const state = desktopState(fakeBrowser({ noApi: true }).env);
    expect(state.status).toBe('unsupported');
    expect(state.canRequest).toBe(false);
    // The explanation must point at what still works, or it reads as "broken".
    expect(state.detail).toMatch(/sound/i);
  });

  it('explains an insecure context instead of reporting a permission problem', () => {
    // A LAN address over plain HTTP: the API is not exposed at all, and no amount
    // of granting permission would change that.
    const state = desktopState(fakeBrowser({ secure: false }).env);
    expect(state.status).toBe('insecure');
    expect(state.canRequest).toBe(false);
    expect(state.detail).toMatch(/https|localhost/i);
  });

  it('offers to ask when permission has not been decided', () => {
    const state = desktopState(fakeBrowser({ permission: 'default' }).env);
    expect(state.status).toBe('default');
    expect(state.canRequest).toBe(true);
  });

  it('does not offer to ask once permission was denied', () => {
    // The browser will not prompt again, so a button that called
    // `requestPermission()` would do nothing and look broken.
    const state = desktopState(fakeBrowser({ permission: 'denied' }).env);
    expect(state.status).toBe('denied');
    expect(state.canRequest).toBe(false);
    expect(state.detail).toMatch(/site settings|padlock/i);
  });

  it('reports granted on a desktop browser', () => {
    const state = desktopState(fakeBrowser({ permission: 'granted' }).env);
    expect(state.status).toBe('granted');
    expect(state.canRequest).toBe(false);
  });

  it('warns that a mobile browser may not be able to show one', () => {
    // The constructor throws there without a service worker, which karucode does
    // not register, so promising it would be a lie.
    const state = desktopState(
      fakeBrowser({ permission: 'granted', mobile: true }).env,
    );
    expect(state.status).toBe('limited');
    expect(state.detail).toMatch(/may not/i);
  });
});

describe('requesting permission', () => {
  it('never asks unless a caller with a gesture asks it to', async () => {
    const fake = fakeBrowser({ permission: 'default' });
    // Constructing the module and reading state must not prompt: importing a module
    // is not a user gesture, and Firefox/Safari refuse a prompt without one.
    desktopState(fake.env);
    expect(fake.requests).toBe(0);
  });

  it('asks and reports the new permission', async () => {
    const fake = fakeBrowser({ permission: 'default' });
    expect(await requestDesktopPermission(fake.env)).toBe('granted');
    expect(fake.requests).toBe(1);
  });

  it('refuses to ask when the browser will not prompt again', async () => {
    const fake = fakeBrowser({ permission: 'denied' });
    expect(await requestDesktopPermission(fake.env)).toBeNull();
    expect(fake.requests).toBe(0);
  });

  it('refuses to ask in an insecure context', async () => {
    const fake = fakeBrowser({ permission: 'default', secure: false });
    expect(await requestDesktopPermission(fake.env)).toBeNull();
    expect(fake.requests).toBe(0);
  });

  it('reports the resulting state when the prompt rejects', async () => {
    const fake = fakeBrowser({ permission: 'default' });
    const ctor = fake.env.NotificationCtor as unknown as {
      requestPermission: () => Promise<NotificationPermission>;
    };
    ctor.requestPermission = () => Promise.reject(new Error('dismissed'));
    expect(await requestDesktopPermission(fake.env)).toBe('default');
  });
});

describe('showing one', () => {
  it('shows only when permission is granted', () => {
    const denied = fakeBrowser({ permission: 'denied' });
    expect(showDesktopNotification(denied.env, note)).toEqual({
      shown: false,
      reason: 'denied',
    });
    expect(denied.shown).toHaveLength(0);

    const granted = fakeBrowser({ permission: 'granted' });
    expect(showDesktopNotification(granted.env, note)).toEqual({ shown: true });
    expect(granted.shown).toHaveLength(1);
    expect(granted.shown[0]?.title).toBe('Run finished');
  });

  it('passes a tag so five alerts replace each other instead of stacking', () => {
    const fake = fakeBrowser({ permission: 'granted' });
    showDesktopNotification(fake.env, { ...note, tag: 'approval' });
    showDesktopNotification(fake.env, { ...note, tag: 'approval' });
    expect(fake.shown[0]?.options.tag).toBe('approval');
    expect(fake.shown.every((n) => n.options.tag === 'approval')).toBe(true);
  });

  it('silences the OS sound when the app already played a cue', () => {
    // One event must never produce two noises.
    const fake = fakeBrowser({ permission: 'granted' });
    showDesktopNotification(fake.env, { ...note, silent: true });
    expect(fake.shown[0]?.options.silent).toBe(true);
  });

  it('carries requireInteraction for something that should not vanish', () => {
    const fake = fakeBrowser({ permission: 'granted' });
    showDesktopNotification(fake.env, { ...note, requireInteraction: true });
    expect(fake.shown[0]?.options.requireInteraction).toBe(true);
  });

  it('runs the click handler and closes the notification', () => {
    const fake = fakeBrowser({ permission: 'granted' });
    const onClick = vi.fn();
    showDesktopNotification(fake.env, { ...note, onClick });
    fake.clickFirst();
    expect(onClick).toHaveBeenCalledTimes(1);
    expect(fake.shown[0]?.closed).toBe(true);
  });

  it('survives a click handler that throws', () => {
    const fake = fakeBrowser({ permission: 'granted' });
    showDesktopNotification(fake.env, {
      ...note,
      onClick: () => {
        throw new Error('focus refused');
      },
    });
    // A failed focus is not worth an error dialog.
    expect(() => fake.clickFirst()).not.toThrow();
  });

  it('reports a constructor that throws instead of propagating it', () => {
    // The documented mobile case. A caller must be able to say "limited" rather
    // than leaving the user wondering why nothing appeared.
    const fake = fakeBrowser({ permission: 'granted', throwOnConstruct: true });
    expect(showDesktopNotification(fake.env, note)).toEqual({
      shown: false,
      reason: 'constructor-threw',
    });
  });

  it('never throws, whatever the browser does', () => {
    const fake = fakeBrowser({ permission: 'granted', throwOnConstruct: true });
    expect(() => showDesktopNotification(fake.env, note)).not.toThrow();
    const noApi = fakeBrowser({ noApi: true });
    expect(() => showDesktopNotification(noApi.env, note)).not.toThrow();
  });
});

describe('the production environment', () => {
  it('reports jsdom honestly: no API', () => {
    const env = browserNotificationEnvironment();
    expect(env.NotificationCtor).toBeUndefined();
    expect(desktopState(env).status).toBe('unsupported');
  });

  it('reads visibility from the document rather than caching it', () => {
    // Focus state must be read at notify time: a run that finishes for another
    // session has to notify even when the tab is in front.
    const env = browserNotificationEnvironment();
    expect(['visible', 'hidden']).toContain(env.visibility());
  });
});
