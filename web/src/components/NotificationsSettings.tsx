/**
 * The Notifications tab of the settings sheet.
 *
 * Three kinds of switch live here and they are deliberately separated in the UI,
 * because they answer different questions:
 *
 * * **server routing** (`config.notifications`) — what the *server* is allowed to
 *   announce, for every browser. `enabled = false` stops the `notification` event at
 *   the source, so nothing downstream has to cooperate. Saving PATCHes the server.
 *   `NotificationsSection`.
 * * **this browser** (`localStorage`) — whether *this device* makes a noise or shows
 *   an OS notification. It describes the device, so it must not travel with the
 *   install. `NotificationsDeviceSection`.
 * * **delivery tuning** (`min_run_secs`, `say_body`) — server-side knobs that most
 *   installs never touch, so they sit behind the tab's Advanced disclosure.
 *   `NotificationsDeliverySection`.
 *
 * The desktop-notification line is read-only state, not a switch: permission is the
 * browser's, and the only action is the one button that may ask for it — from a
 * gesture, which is why it is here and not on mount.
 *
 * Each section renders as a `settings-section` so they compose inside a tab panel
 * exactly like the hand-written sections around them.
 */

import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import type { Config, NotificationsConfig, NotificationsPatch } from '../api/types';
import {
  desktopState,
  requestDesktopPermission,
  browserNotificationEnvironment,
  type DesktopState,
} from '../lib/notifyDesktop';
import { CUE_LABELS, CUE_NAMES, type CueName } from '../lib/notifySound';
import * as notifyWiring from '../state/notifyWiring';
import * as store from '../state/store';
import { Toggle } from './Toggle';

/** The per-event switches, in the order the server documents them. */
const EVENT_SWITCHES: { key: keyof NotificationsPatch; label: string; description: string }[] = [
  {
    key: 'run_finished',
    label: 'A run finished',
    description: 'Tell me when the agent has finished the work it was asked to do.',
  },
  {
    key: 'needs_input',
    label: 'Waiting for you',
    description:
      'An approval is pending, or a run stopped at a configured limit and can continue.',
  },
  {
    key: 'error',
    label: 'A run failed',
    description: 'A run failed, was interrupted by a restart, or a subagent failed.',
  },
  {
    key: 'info',
    label: 'Plugin messages',
    description: 'Notifications a plugin raises as information rather than as a problem.',
  },
];

export interface NotificationsSectionProps {
  config: Config | null | undefined;
  /** Settings are read-only (a locked install). */
  disabled: boolean;
  /** Save the server-side switches through `PATCH /api/config`. */
  onSave: (patch: NotificationsPatch) => Promise<boolean>;
}

/** `config.notifications` with the documented defaults, for an older server. */
function withDefaults(config: NotificationsConfig | undefined): NotificationsConfig {
  return (
    config ?? {
      enabled: true,
      run_finished: true,
      needs_input: true,
      error: true,
      info: true,
      plugin: true,
      min_run_secs: 0,
      say_body: true,
      max_min_run_secs: 3600,
      kinds: ['run_finished', 'needs_input', 'error', 'info'],
    }
  );
}

export function NotificationsSection({
  config,
  disabled,
  onSave,
}: NotificationsSectionProps): JSX.Element {
  const routing = withDefaults(config?.notifications);
  // A local copy so a toggle flips immediately; the save is debounced through the
  // parent's form, which is why this component does not own a submit button.
  const [draft, setDraft] = useState<NotificationsConfig>(routing);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-sync when the server's copy changes (a save elsewhere, a reload).
  useEffect(() => {
    setDraft(routing);
  }, [
    routing.enabled,
    routing.run_finished,
    routing.needs_input,
    routing.error,
    routing.info,
    routing.plugin,
    routing.min_run_secs,
    routing.say_body,
  ]);

  const persist = async (patch: NotificationsPatch): Promise<void> => {
    setDraft((current) => ({ ...current, ...patch }) as NotificationsConfig);
    setSaving(true);
    setError(null);
    const ok = await onSave(patch);
    setSaving(false);
    if (!ok) {
      // Put the switch back rather than leaving the UI claiming a setting the
      // server refused.
      setDraft(routing);
      setError('The server refused that change. It is still what it was before.');
    }
  };

  return (
    <section class="settings-section" aria-labelledby="settings-notifications">
      <div class="settings-section-head">
        <h3 id="settings-notifications">Notifications</h3>
        <span class="settings-section-note">
          What the server announces at all, for every browser
        </span>
      </div>

      {error ? (
        <p class="field-error" role="alert">
          {error}
        </p>
      ) : null}

      <Toggle
        id="notify-enabled"
        checked={draft.enabled}
        disabled={disabled || saving}
        label="Notify me about runs"
        description="Off stops the server sending notifications at all — for every browser, not just this one."
        onChange={(checked) => void persist({ enabled: checked })}
      />

      {/* The per-event switches are meaningless while the master switch is off,
          so they are disabled rather than hidden: the user can see what they
          would get back. */}
      <div class="setting-row setting-row-stack">
        <div class="setting-copy">
          <span class="setting-name">Which events</span>
          <span class="setting-hint">
            {draft.enabled
              ? 'Each kind can be silenced on its own.'
              : 'Everything is off while notifications are disabled above.'}
          </span>
        </div>
        <div class="toggle-group">
          {EVENT_SWITCHES.map((item) => (
            <Toggle
              key={String(item.key)}
              id={`notify-${String(item.key)}`}
              checked={Boolean(draft[item.key as keyof NotificationsConfig])}
              disabled={disabled || saving || !draft.enabled}
              label={item.label}
              description={item.description}
              onChange={(checked) => void persist({ [item.key]: checked })}
            />
          ))}
        </div>
      </div>

      <Toggle
        id="notify-plugin"
        checked={draft.plugin}
        disabled={disabled || saving || !draft.enabled}
        label="Let plugins raise notifications"
        description="A plugin can ask for your attention (and observe notifications with on_notify). Off refuses both, whatever the plugin sends."
        onChange={(checked) => void persist({ plugin: checked })}
      />
    </section>
  );
}

/**
 * The part of the notification settings that describes *this device*.
 *
 * Nothing here PATCHes the server: the switches are browser preferences, stored in
 * localStorage, and they apply only to the tab you are looking at.
 */
export function NotificationsDeviceSection(): JSX.Element {
  const [desktop, setDesktop] = useState<DesktopState>(() =>
    desktopState(browserNotificationEnvironment()),
  );
  const [sound, setSound] = useState(store.soundEnabled.value);
  const [volume, setVolume] = useState(store.soundVolume.value);
  const [desktopWanted, setDesktopWanted] = useState(store.desktopEnabled.value);
  const [preview, setPreview] = useState<CueName | null>(null);

  return (
    <section class="settings-section" aria-labelledby="settings-notifications-browser">
      <div class="settings-section-head">
        <h3 id="settings-notifications-browser">This browser</h3>
        <span class="settings-section-note">
          Stored on this device only; other browsers are unaffected
        </span>
      </div>

      <Toggle
        id="notify-sound"
        checked={sound}
        label="Play a sound"
        description="Independent of the notification permission: the cue plays whether or not the browser allows notifications."
        onChange={(checked) => {
          setSound(checked);
          store.persistSound(checked);
          notifyWiring.syncSoundPreferences();
        }}
      />

      <div class="setting-row">
        <div class="setting-copy">
          <span class="setting-name">Volume</span>
          <span class="setting-hint">Applies to this browser.</span>
        </div>
        <input
          id="notify-volume"
          class="input-range"
          type="range"
          min="0"
          max="100"
          step="5"
          value={String(Math.round(volume * 100))}
          disabled={!sound}
          aria-label="Notification sound volume"
          onInput={(event) => {
            const next = Number(event.currentTarget.value) / 100;
            setVolume(next);
            store.persistVolume(next);
            notifyWiring.syncSoundPreferences();
          }}
        />
      </div>

      <div class="setting-row setting-row-stack">
        <div class="setting-copy">
          <span class="setting-name">Preview a cue</span>
          <span class="setting-hint">
            Each kind has its own sound. A preview plays even while sound is muted, so
            the button can answer whether it works.
          </span>
        </div>
        <div class="cue-buttons">
          {CUE_NAMES.map((cue) => (
            <button
              key={cue}
              type="button"
              class="panel-action-btn"
              onClick={() => {
                setPreview(cue);
                void notifyWiring.previewCue(cue).finally(() => setPreview(null));
              }}
            >
              {CUE_LABELS[cue]}
              {preview === cue ? ' …' : ''}
            </button>
          ))}
        </div>
      </div>

      <Toggle
        id="notify-desktop"
        checked={desktopWanted}
        label="Desktop notifications"
        description={desktop.detail}
        disabled={!desktop.canRequest && desktop.status !== 'granted' && desktop.status !== 'limited'}
        onChange={(checked) => {
          setDesktopWanted(checked);
          store.persistDesktopEnabled(checked);
        }}
      />

      {desktop.canRequest ? (
        <div class="setting-row">
          <div class="setting-copy">
            <span class="setting-name">Browser permission</span>
            <span class="setting-hint">
              The browser has not been asked yet. It will only ask because of this
              button — never on its own.
            </span>
          </div>
          <button
            type="button"
            class="panel-action-btn"
            onClick={() => {
              void (async () => {
                await requestDesktopPermission(browserNotificationEnvironment());
                // Re-read rather than assuming: the user may have dismissed it.
                setDesktop(desktopState(browserNotificationEnvironment()));
                if (browserNotificationEnvironment().NotificationCtor?.permission === 'granted') {
                  // Granting it is a clear signal that it is wanted.
                  setDesktopWanted(true);
                  store.persistDesktopEnabled(true);
                }
              })();
            }}
          >
            Allow notifications
          </button>
        </div>
      ) : (
        <p class="field-hint" data-testid="notify-desktop-state">
          {desktop.detail}
        </p>
      )}
    </section>
  );
}

/**
 * Server-side delivery tuning: the two knobs that decide *whether* a notification
 * is worth sending, rather than which events may be sent.
 *
 * These live behind the tab's Advanced disclosure because the defaults are right
 * for nearly every install, and both answers ("notify me about everything" and
 * "skip the noise") are the default for someone who never opens this.
 */
export function NotificationsDeliverySection({
  config,
  disabled,
  onSave,
}: NotificationsSectionProps): JSX.Element {
  const routing = withDefaults(config?.notifications);
  const [draft, setDraft] = useState<NotificationsConfig>(routing);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setDraft(routing);
  }, [routing.min_run_secs, routing.say_body, routing.enabled, routing.max_min_run_secs]);

  const persist = async (patch: NotificationsPatch): Promise<void> => {
    setDraft((current) => ({ ...current, ...patch }) as NotificationsConfig);
    setSaving(true);
    setError(null);
    const ok = await onSave(patch);
    setSaving(false);
    if (!ok) {
      setDraft(routing);
      setError('The server refused that change. It is still what it was before.');
    }
  };

  return (
    <section class="settings-section" aria-labelledby="settings-notifications-delivery">
      <div class="settings-section-head">
        <h3 id="settings-notifications-delivery">Delivery</h3>
        <span class="settings-section-note">Server-side; applies to every browser</span>
      </div>

      {error ? (
        <p class="field-error" role="alert">
          {error}
        </p>
      ) : null}

      <div class="setting-row">
        <div class="setting-copy">
          <span class="setting-name">Minimum run length</span>
          <span class="setting-hint">
            Skip notifications for runs shorter than this. 0 means every run notifies.
          </span>
        </div>
        <input
          id="notify-min-run"
          class="input mono input-inline"
          type="number"
          min="0"
          max={String(routing.max_min_run_secs)}
          step="1"
          value={String(draft.min_run_secs)}
          disabled={disabled || saving || !draft.enabled}
          aria-describedby="notify-min-run-hint"
          onInput={(event) => {
            // Local only until blur: PATCHing on every keystroke would send "3",
            // "36", "360" while the user types.
            const next = Number(event.currentTarget.value);
            if (Number.isFinite(next)) {
              setDraft((current) => ({ ...current, min_run_secs: Math.max(0, next) }));
            }
          }}
          onBlur={() => {
            const clamped = Math.min(
              routing.max_min_run_secs,
              Math.max(0, Math.round(draft.min_run_secs)),
            );
            if (clamped !== routing.min_run_secs) void persist({ min_run_secs: clamped });
          }}
        />
      </div>
      <p class="field-hint" id="notify-min-run-hint">
        Seconds. The ceiling is {routing.max_min_run_secs}.
      </p>

      <Toggle
        id="notify-say-body"
        checked={draft.say_body}
        disabled={disabled || saving || !draft.enabled}
        label="Include the start of the answer"
        description="Off sends the title only, which keeps model output off a lock screen."
        onChange={(checked) => void persist({ say_body: checked })}
      />
    </section>
  );
}
