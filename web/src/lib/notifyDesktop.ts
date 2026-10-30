/**
 * Desktop notifications: permission, capability detection and the one place a
 * notification is allowed to be shown.
 *
 * Three facts shape this module, all of them checked against a real browser rather
 * than assumed:
 *
 * 1. **`new Notification()` needs a secure context.** `https:`, `http://127.0.0.1`
 *    and `http://localhost` qualify; a LAN address over plain HTTP does not. So the
 *    first question is not "has the user allowed this" but "can this page use it at
 *    all", and the answer is shown in settings rather than discovered by a
 *    notification that never appears.
 * 2. **Permission must be requested from a user gesture.** Firefox (≥72) and Safari
 *    refuse otherwise, and Chrome shows a quieter prompt. Nothing here asks on load;
 *    `requestPermission()` is only ever called from the settings button.
 * 3. **Most mobile browsers throw from the constructor** — persistent notifications
 *    there need a service worker, which karucode does not register. So a failure is
 *    expected on mobile and is reported as a capability, not an error.
 *
 * The module is deliberately ignorant of sounds: sound works whether or not any of
 * this is permitted, which is the point of keeping them apart.
 */

/** Why desktop notifications are or are not usable here. */
export type DesktopStatus =
  /** The API exists, the page is a secure context, and permission is granted. */
  | 'granted'
  /** Usable, but the user has not answered yet. Only a button may ask. */
  | 'default'
  /** The user (or a policy) refused. Asking again does nothing; say how to undo it. */
  | 'denied'
  /** No `Notification` in this browser. */
  | 'unsupported'
  /** The page is not a secure context, so the API is unavailable regardless. */
  | 'insecure'
  /** The browser may throw from the constructor (a mobile browser without a SW). */
  | 'limited';

/** A plain-language explanation for the settings screen. */
export interface DesktopState {
  status: DesktopStatus;
  /** One sentence, ready to render. */
  detail: string;
  /** Whether a button may usefully call `requestPermission()`. */
  canRequest: boolean;
}

/** The parts of the DOM this module touches, injectable for tests. */
export interface NotificationEnvironment {
  /** `window.Notification`, or undefined where the API does not exist. */
  NotificationCtor: NotificationCtor | undefined;
  /** `window.isSecureContext`. */
  isSecureContext: boolean;
  /** `document.visibilityState`, read at notify time. */
  visibility: () => DocumentVisibilityState;
  /** Whether the browser is one where the constructor is known to throw. */
  isMobile: () => boolean;
}

/** The constructor subset used here. */
export interface NotificationCtor {
  readonly permission: NotificationPermission;
  requestPermission(): Promise<NotificationPermission>;
  new (title: string, options?: NotificationOptions): NotificationHandle;
}

/** The instance subset used here. */
export interface NotificationHandle {
  onclick: ((event: Event) => void) | null;
  close(): void;
}

/** What a caller wants shown. */
export interface DesktopNotification {
  title: string;
  body: string;
  /** Replace rather than stack: one tag per kind. */
  tag: string;
  /** Keep it on screen until dismissed. */
  requireInteraction: boolean;
  /** Suppress the OS sound because the app already played a cue. */
  silent: boolean;
  /** Called when the notification is clicked. */
  onClick?: () => void;
}

/**
 * Detect whether this browser can show a desktop notification.
 *
 * Pure and synchronous, so the settings screen can render the right explanation on
 * first paint instead of after a probe.
 */
export function desktopState(env: NotificationEnvironment): DesktopState {
  if (!env.NotificationCtor) {
    return {
      status: 'unsupported',
      detail: 'This browser has no notification API. The in-app badge and the sound still work.',
      canRequest: false,
    };
  }
  if (!env.isSecureContext) {
    // Not a permission problem and not fixable from here: the API is simply not
    // exposed on an insecure origin. karucode speaks plain HTTP and expects TLS in
    // front, so this is the LAN-address case.
    return {
      status: 'insecure',
      detail:
        'Desktop notifications need a secure context. Open karucode over HTTPS, or on localhost.',
      canRequest: false,
    };
  }
  const permission = env.NotificationCtor.permission;
  if (permission === 'granted') {
    return {
      status: env.isMobile() ? 'limited' : 'granted',
      detail: env.isMobile()
        ? 'This browser may not show desktop notifications from a page; the in-app badge and the sound still work.'
        : 'Desktop notifications are on.',
      canRequest: false,
    };
  }
  if (permission === 'denied') {
    return {
      status: 'denied',
      detail:
        'Notifications are blocked for this site. The browser will not ask again — change it in the site settings (the padlock in the address bar).',
      canRequest: false,
    };
  }
  return {
    status: 'default',
    detail: 'Not asked yet. Allow desktop notifications to be told about a run while you are elsewhere.',
    canRequest: true,
  };
}

/**
 * Ask for permission.
 *
 * Must be called from a user gesture. Returns the resulting state so the caller can
 * re-render without re-reading the browser.
 */
export async function requestDesktopPermission(
  env: NotificationEnvironment,
): Promise<NotificationPermission | null> {
  const state = desktopState(env);
  if (!state.canRequest || !env.NotificationCtor) return null;
  try {
    return await env.NotificationCtor.requestPermission();
  } catch {
    // Some browsers reject instead of resolving (a dismissed prompt, a policy).
    // The state after the attempt is what matters, so report it.
    return env.NotificationCtor.permission;
  }
}

/**
 * Show one notification, or explain why not.
 *
 * Never throws: a notification is a courtesy. `silent` is set by the caller when the
 * app already played a sound cue, so one event never produces two noises.
 */
export function showDesktopNotification(
  env: NotificationEnvironment,
  notification: DesktopNotification,
): { shown: boolean; reason?: DesktopStatus | 'constructor-threw' } {
  const state = desktopState(env);
  if (state.status !== 'granted' && state.status !== 'limited') {
    return { shown: false, reason: state.status };
  }
  if (!env.NotificationCtor) return { shown: false, reason: 'unsupported' };
  try {
    const handle = new env.NotificationCtor(notification.title, {
      body: notification.body,
      // `tag` replaces a previous notification with the same tag instead of
      // stacking a second one: five approvals must not become five toasts.
      tag: notification.tag,
      requireInteraction: notification.requireInteraction,
      silent: notification.silent,
    });
    if (notification.onClick) {
      const onClick = notification.onClick;
      handle.onclick = () => {
        // The click handler must not be able to break the page.
        try {
          onClick();
        } catch {
          // Ignore: a failed focus is not worth an error dialog.
        }
        handle.close();
      };
    }
    return { shown: true };
  } catch {
    // The documented mobile case: the constructor throws where a service worker
    // would be needed. Reported so settings can say "limited" rather than leaving
    // the user to wonder.
    return { shown: false, reason: 'constructor-threw' };
  }
}

/** The production environment. */
export function browserNotificationEnvironment(): NotificationEnvironment {
  const ctor =
    typeof Notification !== 'undefined'
      ? (Notification as unknown as NotificationCtor)
      : undefined;
  return {
    NotificationCtor: ctor,
    isSecureContext: typeof window !== 'undefined' ? window.isSecureContext : false,
    visibility: () =>
      typeof document !== 'undefined' ? document.visibilityState : 'visible',
    isMobile: () =>
      typeof navigator !== 'undefined' &&
      /android|iphone|ipad|ipod|mobile/i.test(
        `${navigator.userAgent ?? ''} ${navigator.platform ?? ''}`,
      ),
  };
}
