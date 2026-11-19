/**
 * Install plugins from a GitHub repository or a local directory, without
 * leaving the browser: the web half of `karucode plugin install`.
 *
 * Shape: source in, preview out (what the source offers, what each plugin
 * asks to touch, whether it builds), then a choice of plugins, then a job the
 * component polls until the server reports it completed — by which point the
 * job has already rescanned the host, so the plugin list above this section
 * shows the new arrivals without a manual click.
 *
 * Self-contained like `AccessSettings`: it owns its own state and calls, so
 * it never touches the sheet's form or its Save button. The only shared-file
 * footprint is the one-line mount in `SettingsSheet.tsx`.
 */
import { useEffect, useRef, useState } from 'preact/hooks';

import { toErrorMessage } from '../api/client';
import {
  cancelPluginInstall,
  getPluginInstallJob,
  previewPluginInstall,
  startPluginInstall,
  type PluginInstallJob,
  type PluginInstallPreview,
} from '../api/endpoints';
import {
  Button,
  Callout,
  Checkbox,
  Field,
  Inline,
  Input,
  Spinner,
  Stack,
  Text,
} from '../ui';

export interface PluginInstallSectionProps {
  /** Called after a job completes, so the plugin list can refresh itself. */
  onInstalled?: () => void;
}

type Phase =
  | { kind: 'idle' }
  | { kind: 'previewing' }
  | { kind: 'choosing'; preview: PluginInstallPreview; source: string }
  | { kind: 'installing'; id: string }
  | { kind: 'done'; job: PluginInstallJob }
  | { kind: 'failed'; message: string; job: PluginInstallJob | null };

const POLL_MS = 700;

function eventLine(event: PluginInstallJob['events'][number]): string {
  switch (event.kind) {
    case 'downloading':
      return `Downloading ${event.url}`;
    case 'note':
      return event.message;
    case 'building':
      return `Building ${event.plugin} (${event.command})`;
    case 'installed':
      return `Installed ${event.plugin} → ${event.path}`;
    case 'done':
      return `${event.installed} plugin${event.installed === 1 ? '' : 's'} installed.`;
  }
}

export function PluginInstallSection({ onInstalled }: PluginInstallSectionProps) {
  const [source, setSource] = useState('');
  const [gitRef, setGitRef] = useState('');
  const [force, setForce] = useState(false);
  const [noBuild, setNoBuild] = useState(false);
  const [adminToken, setAdminToken] = useState('');
  const [phase, setPhase] = useState<Phase>({ kind: 'idle' });
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [job, setJob] = useState<PluginInstallJob | null>(null);
  const pollTimer = useRef<number | null>(null);
  const notified = useRef(false);
  // The refresh callback, kept in a ref so its identity (an inline arrow in
  // the settings tab, new every render) cannot restart the poll loop mid-job.
  const installedRef = useRef(onInstalled);
  installedRef.current = onInstalled;

  // The poll loop: while a job runs, sample it and move to a terminal phase.
  // One interval, cleared on unmount and on every terminal transition.
  useEffect(() => {
    if (phase.kind !== 'installing') {
      if (pollTimer.current !== null) {
        window.clearTimeout(pollTimer.current);
        pollTimer.current = null;
      }
      return;
    }
    let cancelled = false;
    const id = phase.id;
    const tick = async () => {
      try {
        const snapshot = await getPluginInstallJob(id, {
          headers: adminToken ? { 'X-Karucode-Admin-Token': adminToken } : undefined,
        });
        if (cancelled) return;
        setJob(snapshot);
        if (snapshot.status === 'running') {
          pollTimer.current = window.setTimeout(tick, POLL_MS);
          return;
        }
        if (snapshot.status === 'completed') {
          setPhase({ kind: 'done', job: snapshot });
          if (!notified.current) {
            notified.current = true;
            installedRef.current?.();
          }
        } else {
          // failed or cancelled: the message is in the job when there is one.
          const message = snapshot.error ?? 'the install was cancelled';
          setError(message);
          setPhase({ kind: 'failed', message, job: snapshot });
        }
      } catch (requestError) {
        if (cancelled) return;
        // A missing job (server restarted) or a network blip: stop polling,
        // keep what was shown, and surface the problem.
        if (pollTimer.current !== null) {
          window.clearTimeout(pollTimer.current);
          pollTimer.current = null;
        }
        setError(toErrorMessage(requestError));
        setPhase({ kind: 'failed', message: String(requestError), job: null });
      }
    };
    pollTimer.current = window.setTimeout(tick, POLL_MS);
    return () => {
      cancelled = true;
      if (pollTimer.current !== null) {
        window.clearTimeout(pollTimer.current);
        pollTimer.current = null;
      }
    };
    // adminToken is read through the closure; a change mid-job is unusual and
    // the loop re-arms on phase anyway.
  }, [phase]);

  const headers = adminToken.trim()
    ? { 'X-Karucode-Admin-Token': adminToken.trim() }
    : undefined;

  const runPreview = async () => {
    const trimmed = source.trim();
    if (!trimmed) {
      setError('Give a source: a GitHub repository (owner/repo, a URL) or a local directory.');
      return;
    }
    setError(null);
    setPhase({ kind: 'previewing' });
    try {
      const preview = await previewPluginInstall(
        {
          source: trimmed,
          ...(gitRef.trim() ? { git_ref: gitRef.trim() } : {}),
        },
        { headers },
      );
      setSelected([]);
      setPhase({ kind: 'choosing', preview, source: trimmed });
      if (preview.plugins.length === 0) {
        setError(
          'The source offers no plugins: expected a root index.toml, a plugin.toml in each ' +
            'plugin directory, or a plugin.toml at the root.',
        );
      }
    } catch (requestError) {
      setError(toErrorMessage(requestError));
      setPhase({ kind: 'idle' });
    }
  };

  const runInstall = async () => {
    if (phase.kind !== 'choosing') return;
    setError(null);
    // An empty selection means "everything the source offers" — the button
    // says exactly that, and the backend treats [] the same way.
    const names = selected;
    try {
      const started = await startPluginInstall(
        {
          source: phase.source,
          ...(names.length ? { names } : {}),
          ...(gitRef.trim() ? { git_ref: gitRef.trim() } : {}),
          force,
          no_build: noBuild,
        },
        { headers },
      );
      notified.current = false;
      setJob(null);
      setPhase({ kind: 'installing', id: started.id });
    } catch (requestError) {
      setError(toErrorMessage(requestError));
    }
  };

  const runCancel = async () => {
    if (phase.kind !== 'installing' || !job) return;
    try {
      await cancelPluginInstall(phase.id, { headers });
    } catch (requestError) {
      // A 409 means the job finished between the poll and the click; the next
      // poll resolves the phase on its own.
      setError(toErrorMessage(requestError));
    }
  };

  const reset = () => {
    if (pollTimer.current !== null) {
      window.clearTimeout(pollTimer.current);
      pollTimer.current = null;
    }
    setPhase({ kind: 'idle' });
    setJob(null);
    setError(null);
    setSelected([]);
  };

  const busy = phase.kind === 'previewing' || phase.kind === 'installing';

  return (
    <section class="settings-section plugin-install-section" aria-labelledby="plugin-install">
      <div class="settings-section-head">
        <h3 id="plugin-install">Install from a repository</h3>
        {phase.kind === 'choosing' || phase.kind === 'done' || phase.kind === 'failed' ? (
          <div class="settings-section-actions">
            <Button variant="secondary" size="sm" onClick={reset}>
              Start over
            </Button>
          </div>
        ) : null}
      </div>
      <Text as="p" size="tiny" tone="muted" class="settings-section-note">
        A GitHub repository ({'owner/repo'}, a URL, {'owner/repo@ref'}) or a local directory, in
        the layout the CLI installer takes: an index.toml or one directory per plugin. The
        install runs the plugins&rsquo; builds and rescans the server when it lands — no restart.
      </Text>

      {error ? (
        <Callout tone="danger" role="alert">
          <Stack gap="3">
            <Text as="p" size="small">
              {error}
            </Text>
            {String(error).includes('admin credential') ? (
              <Field label="Admin token" htmlFor="plugin-install-admin">
                <Input
                  id="plugin-install-admin"
                  type="password"
                  value={adminToken}
                  onInput={(e) => setAdminToken(e.currentTarget.value)}
                  placeholder="X-Karucode-Admin-Token"
                  autoComplete="off"
                />
              </Field>
            ) : null}
          </Stack>
        </Callout>
      ) : null}

      {phase.kind === 'idle' || phase.kind === 'previewing' ? (
        <Stack gap="5" class="plugin-install-form">
          <Field label="Source" htmlFor="plugin-install-source" class="field">
            <Input
              id="plugin-install-source"
              type="text"
              value={source}
              onInput={(e) => setSource(e.currentTarget.value)}
              placeholder="owner/plugin-repo, https://github.com/owner/repo, or /path/to/dir"
              spellCheck={false}
            />
          </Field>
          <Field label="Ref" htmlFor="plugin-install-ref" class="field">
            <Input
              id="plugin-install-ref"
              type="text"
              value={gitRef}
              onInput={(e) => setGitRef(e.currentTarget.value)}
              placeholder="branch, tag or commit (default HEAD)"
              spellCheck={false}
            />
          </Field>
          <Inline>
            <Button
              variant="secondary"
              loading={phase.kind === 'previewing'}
              onClick={runPreview}
              disabled={busy}
              title="Fetch the source and list its plugins without installing"
            >
              {phase.kind === 'previewing' ? 'Looking…' : 'Preview'}
            </Button>
          </Inline>
        </Stack>
      ) : null}

      {phase.kind === 'choosing' ? (
        <Stack gap="4" class="plugin-install-choose">
          <Text as="p" size="message" tone="secondary" mono class="plugin-install-source-line">
            {`${phase.preview.source}${phase.preview.resolved ? ` @ ${phase.preview.resolved}` : ''} — ${phase.preview.plugins.length} plugin${phase.preview.plugins.length === 1 ? '' : 's'}`}
          </Text>
          {phase.preview.notes.map((note, index) => (
            <Callout key={index} tone="warn" role="status">
              {note}
            </Callout>
          ))}
          <ul class="plugin-install-list">
            {phase.preview.plugins.map((plugin) => {
              const checked = selected.includes(plugin.name);
              const toggle = () =>
                setSelected((names) =>
                  checked ? names.filter((n) => n !== plugin.name) : [...names, plugin.name],
                );
              return (
                <li key={plugin.name} class="plugin-install-row">
                  <Checkbox
                    checked={checked}
                    onChange={toggle}
                    disabled={busy}
                    label={plugin.name}
                    description={plugin.description ?? undefined}
                  />
                  <Text size="label" tone="secondary" class="plugin-install-meta">
                    {[
                      plugin.builds ? 'builds' : null,
                      plugin.permissions.length > 0
                        ? `asks: ${plugin.permissions.join(', ')}`
                        : 'no permissions declared',
                      plugin.already_installed ? 'installed (needs force)' : null,
                    ]
                      .filter(Boolean)
                      .join(' · ')}
                  </Text>
                </li>
              );
            })}
          </ul>
          <Inline gap="7" wrap class="plugin-install-options">
            <Checkbox
              checked={force}
              onChange={setForce}
              disabled={busy}
              label="Replace existing installs"
            />
            <Checkbox
              checked={noBuild}
              onChange={setNoBuild}
              disabled={busy}
              label="Skip builds"
            />
          </Inline>
          <Inline gap="4" class="plugin-install-actions">
            <Button variant="primary" onClick={runInstall} disabled={busy}>
              Install {selected.length > 0 ? selected.join(', ') : 'everything'}
            </Button>
            <Button variant="secondary" onClick={reset} disabled={busy}>
              Cancel
            </Button>
          </Inline>
        </Stack>
      ) : null}

      {phase.kind === 'installing' || phase.kind === 'done' || phase.kind === 'failed' ? (
        <div class="plugin-install-progress" role="log" aria-live="polite">
          {phase.kind === 'installing' ? (
            <Inline gap="5" align="center" class="plugin-install-running">
              <Spinner size={14} />
              <Text size="body">Installing…</Text>
              {job ? (
                <Text size="tiny" tone="muted">
                  {Math.round(job.elapsed_ms / 1000)}s
                </Text>
              ) : null}
              <Button variant="secondary" size="sm" onClick={runCancel} disabled={!job}>
                Stop
              </Button>
            </Inline>
          ) : null}
          {(job?.events ?? []).map((event, index) => (
            <Text
              as="p"
              size="label"
              mono
              tone={index === 0 ? 'default' : 'secondary'}
              class="plugin-install-event"
              key={index}
            >
              {eventLine(event)}
            </Text>
          ))}
          {phase.kind === 'done' && job?.result ? (
            <Text as="p" size="body" weight="semibold" class="plugin-install-result">
              {job.result.rescan?.added?.length
                ? `Added: ${job.result.rescan.added.join(', ')} — ready, no restart needed.`
                : 'Installed. The plugins list above has the details.'}
            </Text>
          ) : null}
          {phase.kind === 'failed' && phase.job === null ? (
            <Text as="p" size="body" weight="semibold" tone="danger" class="plugin-install-result">
              The job is no longer available.
            </Text>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}
