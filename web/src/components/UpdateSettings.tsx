/**
 * The Updates tab of the settings sheet.
 *
 * Three blocks, because they answer three different questions:
 *
 * * **This build** — what is running: the build number (the commit count), the
 *   commit and its date, the target, and whether the binary was built from a dirty
 *   tree. This is the screen an operator opens before reporting a bug.
 * * **Update** — what the last check found, what changed, and the one button that
 *   installs it. When the process may not write its own binary (a root-owned
 *   `/usr/local/bin`, the normal systemd install) the button is replaced with the
 *   exact `sudo` command, because a button that always fails is worse than a
 *   sentence that says why.
 * * **Automatic checks** — the `[updates]` switches. Checking is a small request
 *   and is on by default; installing on its own is not, and cannot be on a
 *   read-only binary anyway.
 *
 * The panel is self-contained (`AccessSettings` is the precedent): it reads
 * `/api/update` itself and PATCHes `[updates]` itself, so it never touches the
 * settings sheet's form or its Save button.
 */

import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { ApiError, toErrorMessage } from '../api/client';
import type { UpdatesPatch } from '../api/types';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Toggle } from './Toggle';

export interface UpdateSettingsProps {
  /** Settings are read-only (a locked install). */
  disabled: boolean;
}

/** Unix milliseconds as a short local date and time. */
function when(ms: number | null | undefined): string {
  if (!ms) return 'never';
  try {
    return new Date(ms).toLocaleString();
  } catch {
    return 'unknown';
  }
}

/** An RFC 3339 string (a commit date) as a short local date. */
function commitDate(text: string | null | undefined): string {
  if (!text || text === 'unknown') return 'unknown';
  const parsed = Date.parse(text);
  if (Number.isNaN(parsed)) return text;
  return new Date(parsed).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}

/** Human label for the install kind, in the tooltip and the badge. */
const INSTALL_LABELS: Record<string, string> = {
  managed: 'installed',
  source: 'build tree',
  container: 'container',
};

export function UpdateSettings({ disabled }: UpdateSettingsProps): JSX.Element {
  const status = store.updateStatus.value;
  const config = store.config.value?.updates;
  const [busy, setBusy] = useState<'check' | 'install' | null>(null);
  const [saving, setSaving] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [intervalDraft, setIntervalDraft] = useState<string>('');
  const [restart, setRestart] = useState(false);
  const [copied, setCopied] = useState(false);

  // The status is cached in the store from boot; refresh it when the tab opens so
  // "checked 6 hours ago" is not stale by a day.
  useEffect(() => {
    void actions.refreshUpdateStatus();
  }, []);

  useEffect(() => {
    if (config) setIntervalDraft(String(Math.round(config.check_secs / 60)));
  }, [config?.check_secs]);

  const malformed = error ?? status?.check.error ?? null;
  const latest = status?.latest ?? null;
  const newer = Boolean(status?.update_available && latest);
  const actionable = status?.install.kind === 'managed' && status.install.writable;
  /**
   * The newest release is already on disk, but this process still runs the old
   * one. Offering "Install" again would re-download the build that is already
   * there; what is left is a restart, and the panel says so.
   */
  const installed = Boolean(
    newer && latest && status?.last_applied && status.last_applied.to === latest.version,
  );

  const save = async (patch: UpdatesPatch, what: string): Promise<void> => {
    setSaving(what);
    setError(null);
    setNotice(null);
    try {
      await actions.saveConfig({ updates: patch });
      setNotice('Saved.');
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === 403) {
        setError(
          'Changing this needs the admin credential (X-Karucode-Admin-Token), which the browser does not store. Edit config.toml or KARUCODE_UPDATE_MANIFEST_URL instead.',
        );
      } else {
        setError(toErrorMessage(cause));
      }
    } finally {
      setSaving(null);
    }
  };

  const checkNow = async (): Promise<void> => {
    setBusy('check');
    setError(null);
    setNotice(null);
    try {
      const next = await actions.runUpdateCheck();
      if (next.update_available) {
        setNotice(`Build ${next.latest?.build ?? next.latest?.version ?? '?'} is available.`);
      } else {
        setNotice('You are running the newest build.');
      }
    } catch (cause) {
      setError(toErrorMessage(cause));
    } finally {
      setBusy(null);
    }
  };

  const installNow = async (): Promise<void> => {
    setBusy('install');
    setError(null);
    setNotice(null);
    try {
      const response = await actions.installUpdate(restart);
      setNotice(response.result.message);
    } catch (cause) {
      setError(toErrorMessage(cause));
    } finally {
      setBusy(null);
    }
  };

  const saveInterval = async (): Promise<void> => {
    const minutes = Number(intervalDraft);
    if (!Number.isFinite(minutes) || minutes <= 0) {
      setError('The check interval has to be a positive number of minutes.');
      return;
    }
    await save({ check_secs: Math.round(minutes * 60) }, 'interval');
  };

  return (
    <>
      <section class="settings-section" aria-labelledby="settings-update-build">
        <div class="settings-section-head">
          <h3 id="settings-update-build">This build</h3>
          <span class="settings-section-note">
            {status ? INSTALL_LABELS[status.install.kind] ?? status.install.kind : 'loading'}
          </span>
        </div>

        {status ? (
          <>
            <div class="update-current">
              <div class="update-build">
                <span class="update-build-number">build {status.current.version}</span>
                <span class="update-build-meta">
                  {status.current.commit}
                  {status.current.commit_date !== 'unknown'
                    ? ` · ${commitDate(status.current.commit_date)}`
                    : ''}
                  {status.current.dirty ? ' · dirty tree' : ''}
                </span>
              </div>
              <dl class="update-facts">
                <div>
                  <dt>Target</dt>
                  <dd>{status.current.target}</dd>
                </div>
                <div>
                  <dt>Channel</dt>
                  <dd>{status.current.channel}</dd>
                </div>
                <div>
                  <dt>Binary</dt>
                  <dd title={status.install.path}>{status.install.path}</dd>
                </div>
              </dl>
            </div>
            <p class="settings-section-note">{status.install.detail}</p>
            {status.last_applied ? (
              <p class="settings-section-note">
                Installed build {status.last_applied.to} on {when(status.last_applied.applied_at)}{' '}
                (was {status.last_applied.from}).
              </p>
            ) : null}
          </>
        ) : (
          <p class="settings-section-note">
            The server did not answer <code>GET /api/update</code>, so this build cannot be named.
            The version is the commit count; <code>karucode --version</code> prints it from a shell.
          </p>
        )}
      </section>

      <section class="settings-section" aria-labelledby="settings-update-available">
        <div class="settings-section-head">
          <h3 id="settings-update-available">Update</h3>
          <div class="settings-section-actions">
            <button
              type="button"
              class="button button-small"
              onClick={() => void checkNow()}
              disabled={busy !== null || !config?.check}
              title={
                config?.check
                  ? 'Ask the release manifest now'
                  : 'Automatic checking is off; the check command uses the manifest URL directly'
              }
            >
              {busy === 'check' ? 'Checking…' : 'Check now'}
            </button>
          </div>
        </div>

        <p class="settings-section-note">
          Last checked {when(status?.check.checked_at)}
          {status?.check.manifest_url ? (
            <>
              {' '}against <code>{status.check.manifest_url}</code>
            </>
          ) : null}
          .
        </p>

        {malformed ? (
          <p class="update-error" role="status">
            {malformed}
          </p>
        ) : null}

        {newer && latest ? (
          <div class="update-available">
            <div class="update-available-head">
              <span class="update-badge">
                build {latest.version} {installed ? 'installed' : 'available'}
              </span>
              <span class="update-build-meta">
                {latest.commit ?? 'unknown commit'}
                {latest.commit_date ? ` · ${commitDate(latest.commit_date)}` : ''}
              </span>
            </div>
            {latest.notes ? <pre class="update-notes">{latest.notes}</pre> : null}
            {latest.url ? (
              <a class="update-link" href={latest.url} target="_blank" rel="noreferrer noopener">
                Release page
              </a>
            ) : null}
            {!latest.has_target_for_this_build ? (
              <p class="update-error">
                This release has no build for {status?.current.target}. Nothing here can install
                it.
              </p>
            ) : installed ? (
              <p class="update-notice">
                Build {latest.build} is installed. Restart the service to run it (
                <code>systemctl restart karucode</code>).
              </p>
            ) : actionable ? (
              <div class="update-actions">
                <button
                  type="button"
                  class="button"
                  onClick={() => void installNow()}
                  disabled={busy !== null || disabled}
                >
                  {busy === 'install' ? 'Installing…' : `Install build ${latest.build}`}
                </button>
                <label class="update-restart">
                  <input
                    type="checkbox"
                    checked={restart}
                    disabled={!status?.restart.supervised || busy !== null}
                    onChange={(event) => setRestart(event.currentTarget.checked)}
                  />
                  Restart the service afterwards
                  {!status?.restart.supervised ? (
                    <span class="setting-hint">
                      {' '}— only possible when a supervisor (systemd) manages this process
                    </span>
                  ) : null}
                </label>
              </div>
            ) : null}
            {!actionable && !installed ? (
              <div class="update-command">
                <span class="setting-hint">
                  {status?.install.kind === 'source'
                    ? 'This binary runs from a build tree. Update it from the checkout:'
                    : status?.install.kind === 'container'
                      ? 'This is a container install. Rebuild the image; nothing here can replace the binary.'
                      : 'This process may not write its own binary. Run this as the owner:'}
                </span>
                <code>{status?.restart.apply_command ?? 'sudo karucode update --apply'}</code>
                {status?.install.kind === 'source' ? (
                  <code>sudo sh packaging/update.sh</code>
                ) : null}
                <button
                  type="button"
                  class="button button-small"
                  onClick={() => {
                    const command = status?.restart.apply_command;
                    if (!command) return;
                    void navigator.clipboard?.writeText(command).then(
                      () => setCopied(true),
                      () => setCopied(false),
                    );
                  }}
                >
                  {copied ? 'Copied' : 'Copy command'}
                </button>
              </div>
            ) : null}
          </div>
        ) : (
          <p class="settings-section-note">
            {status?.latest
              ? `Build ${status.latest.build} is the newest release.`
              : 'No check has found a release yet.'}
          </p>
        )}

        {status?.staged && !newer ? (
          <p class="settings-section-note">
            Build {status.staged.build} is staged and verified at <code>{status.staged.path}</code>.
          </p>
        ) : null}
        {notice ? <p class="update-notice">{notice}</p> : null}
      </section>

      <section class="settings-section" aria-labelledby="settings-update-auto">
        <div class="settings-section-head">
          <h3 id="settings-update-auto">Automatic checks</h3>
        </div>
        <Toggle
          id="update-check"
          checked={config?.check ?? true}
          disabled={disabled || saving !== null}
          label="Check for updates automatically"
          description="One small request to the release manifest on startup and on a timer. Off sends nothing at all, which is what an air-gapped install wants."
          onChange={(checked) => void save({ check: checked }, 'check')}
        />
        <div class="setting-row">
          <div class="setting-copy">
            <span class="setting-name">Check every (minutes)</span>
            <span class="setting-hint">
              Between {Math.round((config?.min_check_secs ?? 300) / 60)} and{' '}
              {Math.round((config?.max_check_secs ?? 604800) / 60)} minutes; clamped by the server.
            </span>
          </div>
          <div class="update-interval">
            <input
              id="update-interval"
              class="input"
              type="number"
              aria-label="Check every (minutes)"
              min={Math.round((config?.min_check_secs ?? 300) / 60)}
              max={Math.round((config?.max_check_secs ?? 604800) / 60)}
              step="5"
              value={intervalDraft}
              disabled={disabled || saving !== null || !config?.check}
              onInput={(event) => setIntervalDraft(event.currentTarget.value)}
            />
            <button
              type="button"
              class="button button-small"
              onClick={() => void saveInterval()}
              disabled={disabled || saving !== null || !config?.check}
            >
              {saving === 'interval' ? 'Saving…' : 'Save'}
            </button>
          </div>
        </div>
        <Toggle
          id="update-auto-apply"
          checked={config?.auto_apply ?? false}
          disabled={disabled || saving !== null}
          label="Install automatically when a newer build is found"
          description="Off by default: installing restarts the service, which ends every run in flight. Only possible when this process may write its own binary."
          onChange={(checked) => void save({ auto_apply: checked }, 'auto_apply')}
        />
        <p class="settings-section-note">
          The manifest URL is <code>{config?.manifest_url ?? 'unset'}</code>. Changing it needs the
          admin credential, so it is read-only here; set <code>updates.manifest_url</code> in
          config.toml, or <code>KARUCODE_UPDATE_MANIFEST_URL</code>.
        </p>
      </section>
    </>
  );
}
