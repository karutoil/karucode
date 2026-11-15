import { useState } from 'preact/hooks';
import type { PluginDiagnostics } from '../api/types';
import { clearPluginToolOutput, decidePluginToolOutput } from '../api/endpoints';
import { toErrorMessage } from '../api/client';

/**
 * The per-plugin tool-output budget: what applies, and the one control that
 * answers a plugin's request for its own.
 *
 * The rule this screen exists to make visible: a budget is the **operator's**
 * decision. An agent implementing or installing a plugin asks for one - naming
 * the plugin, the exact byte value and why - and the answer given here is the
 * only thing that creates it. Nothing is ever set automatically, a request
 * grants nothing until it is answered, and a refusal is remembered.
 *
 * The number shown is the *effective* one: the same ceiling the runtime applies
 * when it truncates a result, so what is on screen is what the model gets.
 */
export interface PluginToolOutputRowProps {
  plugin: PluginDiagnostics;
  /** Re-read the plugin list after an answer or a clear. */
  onChanged: () => void;
}

const MIN_BYTES = 2_000;
const MAX_BYTES = 1_000_000;

/** `24,000` — a byte count the operator can compare at a glance. */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes)) return '—';
  return Math.round(bytes).toLocaleString('en-US');
}

/** Unix milliseconds → `3 Feb 2026, 14:05`, or `—`. */
export function formatWhen(at: number | null | undefined): string {
  if (at === null || at === undefined || !Number.isFinite(at) || at <= 0) return '—';
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return '—';
  const day = date.getDate();
  const month = date.toLocaleString('en-US', { month: 'short' });
  const time = date.toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit' });
  return `${day} ${month} ${date.getFullYear()}, ${time}`;
}

/**
 * Why a pending ask says what it says.
 *
 * A tool that returns whole features or documents wants a much larger budget
 * than the 24,000-byte default; a tool that returns a one-line answer wants a
 * smaller one. This is a *hint for the operator*, never a value the host picks.
 */
export function suggestedBytes(requested: number): number {
  return Math.min(MAX_BYTES, Math.max(MIN_BYTES, Math.round(requested)));
}

export function PluginToolOutputRow({ plugin, onChanged }: PluginToolOutputRowProps) {
  const budget = plugin.tool_output;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [adminToken, setAdminToken] = useState('');
  const [needsAdmin, setNeedsAdmin] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');

  if (!budget) return null;

  const pending = budget.pending ?? null;
  const decision = budget.granted ? 'granted' : null;
  const value = editing ? draft : String(pending?.bytes ?? budget.granted_bytes ?? budget.effective_bytes);

  const answer = (grant: boolean): void => {
    const bytes = Number.parseInt(value, 10);
    if (grant && (!Number.isFinite(bytes) || bytes < MIN_BYTES || bytes > budget.ceiling_bytes)) {
      setError(
        `a budget must be between ${formatBytes(MIN_BYTES)} and ${formatBytes(
          budget.ceiling_bytes,
        )} bytes`,
      );
      return;
    }
    setBusy(true);
    setError(null);
    decidePluginToolOutput(
      plugin.name,
      grant ? 'grant' : 'refuse',
      grant ? bytes : null,
      '',
      adminToken || undefined,
    )
      .then(() => {
        setEditing(false);
        setNeedsAdmin(false);
        onChanged();
      })
      .catch((cause: unknown) => {
        setNeedsAdmin(true);
        setError(toErrorMessage(cause));
      })
      .finally(() => setBusy(false));
  };

  const clear = (): void => {
    setBusy(true);
    setError(null);
    clearPluginToolOutput(plugin.name, adminToken || undefined)
      .then(() => {
        setEditing(false);
        setNeedsAdmin(false);
        onChanged();
      })
      .catch((cause: unknown) => {
        setNeedsAdmin(true);
        setError(toErrorMessage(cause));
      })
      .finally(() => setBusy(false));
  };

  return (
    <div class="plugin-budget" data-plugin={plugin.name}>
      <span class="diagnostics-meta">
        Output budget {formatBytes(budget.effective_bytes)} B{' '}
        {budget.granted
          ? `(granted${budget.decided_by ? ` by ${budget.decided_by}` : ''}${
              budget.decided_at ? `, ${formatWhen(budget.decided_at)}` : ''
            })`
          : `(global default${budget.declared_bytes ? `; a config value of ${formatBytes(budget.declared_bytes)} B needs a grant` : ''})`}
      </span>
      {pending ? (
        <div class="plugin-budget-ask" role="status">
          <p>
            <strong>{plugin.name}</strong> asks for {formatBytes(pending.bytes)} bytes per tool result
            {pending.reason ? ` — “${pending.reason}”` : ''}. Allow?
          </p>
          <label class="sr-only" for={`budget-${plugin.name}`}>
            Bytes to allow
          </label>
          <input
            id={`budget-${plugin.name}`}
            type="number"
            min={MIN_BYTES}
            max={budget.ceiling_bytes}
            step={1000}
            value={value}
            onInput={(event) => {
              setEditing(true);
              setDraft((event.target as HTMLInputElement).value);
            }}
            disabled={busy}
          />
          <button type="button" class="button button-small" onClick={() => answer(true)} disabled={busy}>
            Allow
          </button>
          <button type="button" class="button button-small" onClick={() => answer(false)} disabled={busy}>
            Refuse
          </button>
        </div>
      ) : decision ? (
        <span class="diagnostics-meta">
          <button
            type="button"
            class="button button-small"
            onClick={() => {
              setEditing(true);
              setDraft(String(budget.granted_bytes ?? budget.effective_bytes));
            }}
            disabled={busy}
          >
            Change
          </button>
          <button type="button" class="button button-small" onClick={clear} disabled={busy}>
            Clear
          </button>
        </span>
      ) : null}
      {editing && !pending ? (
        <div class="plugin-budget-ask">
          <label class="sr-only" for={`budget-${plugin.name}`}>
            Bytes to allow
          </label>
          <input
            id={`budget-${plugin.name}`}
            type="number"
            min={MIN_BYTES}
            max={budget.ceiling_bytes}
            step={1000}
            value={value}
            onInput={(event) => setDraft((event.target as HTMLInputElement).value)}
            disabled={busy}
          />
          <button type="button" class="button button-small" onClick={() => answer(true)} disabled={busy}>
            Grant
          </button>
        </div>
      ) : null}
      {needsAdmin ? (
        <input
          type="password"
          placeholder="Admin token"
          value={adminToken}
          onInput={(event) => setAdminToken((event.target as HTMLInputElement).value)}
          aria-label="Admin token"
        />
      ) : null}
      {error ? (
        <span class="diagnostics-error" role="alert">
          {error}
        </span>
      ) : null}
    </div>
  );
}