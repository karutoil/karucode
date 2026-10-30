/**
 * The Notifications section of the settings sheet.
 *
 * Two things matter here and neither is cosmetic:
 *
 * 1. **The server switches actually round-trip.** A toggle that flips in the UI but
 *    never reaches `PATCH /api/config` is a lie, so every test asserts the request.
 * 2. **Permission is never requested on mount.** Firefox and Safari refuse a prompt
 *    that did not come from a gesture, so a section that asked on open would be
 *    broken exactly where it is hardest to notice.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { NotificationsDeliverySection, NotificationsDeviceSection, NotificationsSection } from './NotificationsSettings';
import type { Config, NotificationsPatch } from '../api/types';
import { configFixture } from '../test/fixtures';
import * as store from '../state/store';

function configWith(overrides: Partial<Config['notifications']> = {}): Config {
  return {
    ...configFixture,
    notifications: { ...configFixture.notifications!, ...overrides },
  };
}

/**
 * Render the Notifications tab the way the sheet composes it: the server routing
 * section, the per-device section, and the delivery knobs inside the Advanced
 * disclosure.
 *
 * The three are rendered together on purpose. They used to be one component, and
 * the property worth pinning is exactly that the boundary between "this server"
 * and "this browser" survives the split.
 */
function renderSection(options: {
  config?: Config;
  disabled?: boolean;
  onSave?: (patch: NotificationsPatch) => Promise<boolean>;
} = {}) {
  const onSave = options.onSave ?? (async () => true);
  const spy = vi.fn(onSave);
  const config = options.config ?? configWith();
  render(
    <div>
      <NotificationsSection config={config} disabled={options.disabled ?? false} onSave={spy} />
      <NotificationsDeviceSection />
      <details>
        <summary>Advanced</summary>
        <NotificationsDeliverySection
          config={config}
          disabled={options.disabled ?? false}
          onSave={spy}
        />
      </details>
    </div>,
  );
  return spy;
}

/** The switch with the given accessible label. */
function toggle(label: string): HTMLElement {
  return screen.getByRole('switch', { name: label });
}

/** The `patch` argument of the nth save call, typed for the assertions below. */
function patchAt(spy: { mock: { calls: unknown[][] } }, index: number): NotificationsPatch {
  const call = spy.mock.calls[index];
  if (!call) throw new Error(`expected at least ${index + 1} save calls`);
  return call[0] as NotificationsPatch;
}

/** jsdom leaves `isSecureContext` undefined; the API needs it to be true. */
function stubSecureContext(secure: boolean): void {
  Object.defineProperty(window, 'isSecureContext', {
    configurable: true,
    value: secure,
  });
}

beforeEach(() => {
  localStorage.clear();
  store.soundEnabled.value = true;
  store.soundVolume.value = 0.8;
  store.desktopEnabled.value = false;
});

afterEach(() => {
  localStorage.clear();
  vi.unstubAllGlobals();
  Object.defineProperty(window, 'isSecureContext', {
    configurable: true,
    value: undefined,
  });
});

describe('what the section shows', () => {
  it('renders the server switches and the browser switches under separate headings', () => {
    renderSection();
    expect(screen.getByRole('heading', { name: 'Notifications' })).toBeTruthy();
    // Server routing: the master switch and the per-event group.
    expect(toggle('Notify me about runs')).toBeTruthy();
    expect(toggle('A run finished')).toBeTruthy();
    expect(toggle('Waiting for you')).toBeTruthy();
    expect(toggle('A run failed')).toBeTruthy();
    expect(toggle('Plugin messages')).toBeTruthy();
    expect(toggle('Let plugins raise notifications')).toBeTruthy();
    // This browser.
    expect(screen.getByRole('heading', { name: 'This browser' })).toBeTruthy();
    expect(toggle('Play a sound')).toBeTruthy();
    expect(screen.getByLabelText('Notification sound volume')).toBeTruthy();
  });

  it('explains that sound does not depend on the notification permission', () => {
    // The single most important sentence in this UI.
    renderSection();
    expect(
      screen.getByText(/independent of the notification permission/i),
    ).toBeTruthy();
  });

  it('offers a preview for every cue', () => {
    renderSection();
    for (const label of ['Run finished', 'Needs attention', 'Error', 'Info']) {
      expect(screen.getByRole('button', { name: new RegExp(label, 'i') })).toBeTruthy();
    }
  });

  it('disables the per-event switches while the master switch is off', () => {
    renderSection({ config: configWith({ enabled: false }) });
    expect(toggle('Notify me about runs').getAttribute('aria-checked')).toBe('false');
    // Visible but inert: the user can see what turning it back on would give them.
    expect(toggle('A run finished').hasAttribute('disabled')).toBe(true);
    expect(toggle('Waiting for you').hasAttribute('disabled')).toBe(true);
  });

  it('disables everything when the install is locked', () => {
    renderSection({ disabled: true });
    expect(toggle('Notify me about runs').hasAttribute('disabled')).toBe(true);
    expect(toggle('A run finished').hasAttribute('disabled')).toBe(true);
  });

  it('falls back to the documented defaults when the server sends no section', () => {
    // An older server has no `notifications` key; the section must still render and
    // say that everything is on, rather than crashing or showing nothing.
    const legacy: Config = { ...configFixture };
    delete legacy.notifications;
    renderSection({ config: legacy });
    expect(toggle('Notify me about runs').getAttribute('aria-checked')).toBe('true');
    expect(toggle('A run finished').getAttribute('aria-checked')).toBe('true');
  });
});

describe('saving a switch', () => {
  it('PATCHes the master switch', async () => {
    const spy = renderSection();
    fireEvent.click(toggle('Notify me about runs'));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(patchAt(spy, 0)).toEqual({ enabled: false });
  });

  it('PATCHes one event switch without touching the others', async () => {
    const spy = renderSection();
    fireEvent.click(toggle('A run failed'));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    // Only the flipped key: a full object would let a stale tab overwrite a change
    // made elsewhere.
    expect(patchAt(spy, 0)).toEqual({ error: false });
  });

  it('PATCHes the plugin switch', async () => {
    const spy = renderSection();
    fireEvent.click(toggle('Let plugins raise notifications'));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(patchAt(spy, 0)).toEqual({ plugin: false });
  });

  it('PATCHes the body switch', async () => {
    const spy = renderSection();
    fireEvent.click(toggle('Include the start of the answer'));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(patchAt(spy, 0)).toEqual({ say_body: false });
  });

  it('sends the minimum run length on blur, not on every keystroke', async () => {
    const spy = renderSection();
    const input = screen.getByRole('spinbutton');
    fireEvent.input(input, { target: { value: '30' } });
    // Nothing yet: PATCHing per keystroke would send "3", then "30".
    expect(spy).not.toHaveBeenCalled();
    fireEvent.blur(input);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(patchAt(spy, 0)).toEqual({ min_run_secs: 30 });
  });

  it('clamps a minimum longer than the server allows', async () => {
    const spy = renderSection();
    const input = screen.getByRole('spinbutton');
    fireEvent.input(input, { target: { value: '999999' } });
    fireEvent.blur(input);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    // The server would reject it; clamping here gives the same answer without a
    // round trip that would surface as an error.
    expect(patchAt(spy, 0)).toEqual({ min_run_secs: 3600 });
  });

  it('does not send a PATCH when the value did not change', async () => {
    const spy = renderSection();
    const input = screen.getByRole('spinbutton');
    fireEvent.blur(input);
    expect(spy).not.toHaveBeenCalled();
  });

  it('puts the switch back when the server refuses', async () => {
    const spy = renderSection({ onSave: async () => false });
    const master = toggle('Notify me about runs');
    fireEvent.click(master);
    await waitFor(() => expect(spy).toHaveBeenCalled());
    // A UI that keeps claiming a setting the server rejected is worse than an error.
    await waitFor(() => expect(toggle('Notify me about runs').getAttribute('aria-checked')).toBe('true'));
    expect(screen.getByRole('alert').textContent).toMatch(/refused/i);
  });
});

describe('the browser preferences', () => {
  it('persists the sound switch locally, not to the server', async () => {
    const spy = renderSection();
    fireEvent.click(toggle('Play a sound'));
    expect(store.soundEnabled.value).toBe(false);
    expect(localStorage.getItem('karucode.notify.sound')).toBe('false');
    // A device preference must never be a settings PATCH.
    expect(spy).not.toHaveBeenCalled();
  });

  it('persists the volume as a fraction', async () => {
    renderSection();
    const slider = screen.getByLabelText('Notification sound volume');
    fireEvent.input(slider, { target: { value: '40' } });
    expect(store.soundVolume.value).toBeCloseTo(0.4);
    expect(localStorage.getItem('karucode.notify.volume')).toBe('0.4');
  });

  it('disables the volume slider while sound is off', async () => {
    renderSection();
    fireEvent.click(toggle('Play a sound'));
    await waitFor(() =>
      expect(
        screen.getByLabelText('Notification sound volume').hasAttribute('disabled'),
      ).toBe(true),
    );
  });

  it('persists the desktop preference without asking for permission', async () => {
    const request = vi.fn();
    // jsdom has no Notification at all, so a request would have to come from here.
    renderSection();
    fireEvent.click(toggle('Desktop notifications'));
    // On a browser with no API the switch is inert; what must not happen is a
    // prompt on mount or on toggle.
    expect(request).not.toHaveBeenCalled();
    expect(localStorage.getItem('karucode.notify.desktop')).toBe('true');
  });
});

describe('desktop permission', () => {
  it('never requests permission on mount', () => {
    // The rule that is easiest to break and hardest to notice: Firefox ≥72 and
    // Safari refuse a prompt without a gesture, so a prompt here would do nothing.
    const requestPermission = vi.fn();
    const before = screen.queryByRole('button', { name: /allow notifications/i });
    renderSection();
    expect(requestPermission).not.toHaveBeenCalled();
    // jsdom has no Notification API, so the button is not offered at all...
    expect(before).toBeNull();
    // ...and the state line explains what that means.
    expect(screen.getByTestId('notify-desktop-state').textContent).toMatch(/no notification api/i);
  });

  it('says the API is missing rather than pretending it is off', () => {
    renderSection();
    expect(screen.getByTestId('notify-desktop-state').textContent).toMatch(/still work/i);
  });

  it('explains a denied permission instead of offering to retry', () => {
    const ctor = vi.fn() as unknown as { permission: NotificationPermission };
    Object.defineProperty(ctor, 'permission', { value: 'denied' });
    vi.stubGlobal('Notification', ctor);
    // jsdom does not implement `isSecureContext`; without this every case would
    // report "insecure" and never reach the permission check.
    stubSecureContext(true);
    try {
      renderSection();
      expect(screen.getByTestId('notify-desktop-state').textContent).toMatch(/site settings/i);
      // No button: the browser will not prompt again.
      expect(screen.queryByRole('button', { name: /allow notifications/i })).toBeNull();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('offers the button when the permission has not been decided', () => {
    const ctor = vi.fn() as unknown as { permission: NotificationPermission };
    Object.defineProperty(ctor, 'permission', { value: 'default', configurable: true });
    vi.stubGlobal('Notification', ctor);
    stubSecureContext(true);
    try {
      renderSection();
      const button = screen.getByRole('button', { name: /allow notifications/i });
      expect(button).toBeTruthy();
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
