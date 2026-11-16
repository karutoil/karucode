/**
 * A plugin's own settings, drawn from the schema it declares.
 *
 * The point of the schema (phase 2, §S1) is that this file never learns a
 * plugin's name. `GET /api/plugins/:name/settings` answers with the declaration
 * *and* the values in force, so one form serves every plugin and a plugin author
 * ships a configurable feature without touching karucode's UI at all.
 *
 * Three rules the rest of the app depends on:
 *
 * * **The server validates; this only collects.** Every value is checked again in
 *   `PATCH /api/plugins/:name/settings`, so a stale tab cannot write nonsense.
 *   The client checks only what a person would find annoying to discover late
 *   (an out-of-range number is refused here before the round trip).
 * * **A secret is never read back.** The server sends `value: null` with
 *   `configured: true`; the field shows a placeholder and only sends a replacement
 *   when one is typed, so opening the form cannot wipe a saved secret.
 * * **Saving restarts that one plugin.** The button says so, because "my setting
 *   did not apply" is the failure people blame the app for.
 */
import { useEffect, useMemo, useState } from 'preact/hooks';
import { apiRequest } from '../api/client';

/** One field as the server declares and fills it. */
export interface PluginSettingField {
  key: string;
  label: string;
  type: 'bool' | 'text' | 'number' | 'select' | 'secret';
  default?: unknown;
  help?: string;
  min?: number | null;
  max?: number | null;
  options?: { value: string; label?: string }[];
  /** The value in force, absent for a secret. */
  value?: unknown;
  /** For a secret: whether one is saved. */
  configured?: boolean;
}

type Draft = Record<string, unknown>;

function initialDraft(fields: PluginSettingField[]): Draft {
  const draft: Draft = {};
  for (const field of fields) {
    // A secret starts empty on purpose: the server never sends its value, and an
    // empty field means "leave what is saved alone".
    if (field.type === 'secret') {
      draft[field.key] = '';
      continue;
    }
    draft[field.key] = field.value ?? field.default ?? null;
  }
  return draft;
}

/** Why a value cannot be sent, or `null` when it can. */
export function fieldProblem(field: PluginSettingField, value: unknown): string | null {
  switch (field.type) {
    case 'number': {
      if (typeof value !== 'number' || Number.isNaN(value)) return 'Enter a number.';
      if (typeof field.min === 'number' && value < field.min) return `Must be at least ${field.min}.`;
      if (typeof field.max === 'number' && value > field.max) return `Must be at most ${field.max}.`;
      return null;
    }
    case 'text':
      if (typeof value !== 'string') return 'Enter some text.';
      if (value.length > 2000) return 'That is longer than 2000 characters.';
      return null;
    case 'secret':
      // An empty secret means "unchanged", so it is always allowed.
      return typeof value === 'string' ? null : 'Enter a value.';
    case 'select':
      if (typeof value !== 'string') return 'Choose one of the options.';
      return (field.options ?? []).some((option) => option.value === value)
        ? null
        : 'Choose one of the options.';
    case 'bool':
      return typeof value === 'boolean' ? null : 'Choose on or off.';
  }
}

/** The body to send: only what changed, and never an empty secret. */
export function changesFor(fields: PluginSettingField[], draft: Draft): Draft {
  const body: Draft = {};
  for (const field of fields) {
    const value = draft[field.key];
    if (field.type === 'secret') {
      if (typeof value === 'string' && value !== '') body[field.key] = value;
      continue;
    }
    if (value !== (field.value ?? field.default ?? null)) body[field.key] = value;
  }
  return body;
}

export function PluginSettingsForm({
  plugin,
  onSaved,
}: {
  plugin: string;
  /** Called after a successful save (the plugin has been restarted). */
  onSaved?: () => void;
}) {
  const [fields, setFields] = useState<PluginSettingField[] | null>(null);
  const [draft, setDraft] = useState<Draft>({});
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let live = true;
    setFields(null);
    setError(null);
    apiRequest<{ settings: PluginSettingField[] }>(
      `/api/plugins/${encodeURIComponent(plugin)}/settings`,
    )
      .then((answer) => {
        if (!live) return;
        const loaded = answer.settings ?? [];
        setFields(loaded);
        setDraft(initialDraft(loaded));
      })
      .catch((cause: unknown) => {
        if (live) setError(cause instanceof Error ? cause.message : 'Could not read the settings.');
      });
    return () => {
      live = false;
    };
  }, [plugin]);

  const problems = useMemo(() => {
    if (!fields) return {};
    const found: Record<string, string> = {};
    for (const field of fields) {
      const problem = fieldProblem(field, draft[field.key]);
      if (problem) found[field.key] = problem;
    }
    return found;
  }, [fields, draft]);

  // A plugin with no settings draws nothing at all: an empty "Settings" heading
  // on every plugin is noise.
  if (fields !== null && fields.length === 0) return null;

  const save = async (): Promise<void> => {
    if (!fields) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const body = changesFor(fields, draft);
      if (Object.keys(body).length === 0) {
        setNotice('Nothing to save.');
        return;
      }
      const answer = await apiRequest<{ restarted: boolean; error?: string | null }>(
        `/api/plugins/${encodeURIComponent(plugin)}/settings`,
        {
          method: 'PATCH',
          // `apiRequest` serialises `body` itself; handing it a string here would
          // send a JSON string containing JSON, which the server rejects as a
          // malformed body.
          body: { settings: body },
        },
      );
      setNotice(
        answer.restarted
          ? 'Saved and restarted.'
          : `Saved, but the plugin did not restart: ${answer.error ?? 'unknown reason'}`,
      );
      // Re-read so the form shows what is now in force rather than what was typed.
      const fresh = await apiRequest<{ settings: PluginSettingField[] }>(
        `/api/plugins/${encodeURIComponent(plugin)}/settings`,
      );
      setFields(fresh.settings ?? []);
      setDraft(initialDraft(fresh.settings ?? []));
      onSaved?.();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not save.');
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="plugin-settings">
      <button
        type="button"
        class="plugin-settings-toggle"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        Settings{fields && fields.length > 0 ? ` (${fields.length})` : ''}
      </button>
      {open ? (
        <div class="plugin-settings-body">
          {fields === null ? (
            <p class="muted small">Loading…</p>
          ) : (
            fields.map((field) => {
              const id = `plugin-${plugin}-${field.key}`;
              const problem = problems[field.key];
              return (
                <div class="plugin-setting" key={field.key}>
                  {field.type === 'bool' ? (
                    <label class="plugin-setting-check" for={id}>
                      <input
                        id={id}
                        type="checkbox"
                        checked={draft[field.key] === true}
                        onChange={(event) =>
                          setDraft((current) => ({
                            ...current,
                            [field.key]: (event.currentTarget as HTMLInputElement).checked,
                          }))
                        }
                      />
                      <span>{field.label}</span>
                    </label>
                  ) : (
                    <>
                      <label class="plugin-setting-label" for={id}>
                        {field.label}
                      </label>
                      {field.type === 'select' ? (
                        <select
                          id={id}
                          class="settings-input"
                          value={String(draft[field.key] ?? '')}
                          onChange={(event) =>
                            setDraft((current) => ({
                              ...current,
                              [field.key]: (event.currentTarget as HTMLSelectElement).value,
                            }))
                          }
                        >
                          {(field.options ?? []).map((option) => (
                            <option value={option.value}>{option.label || option.value}</option>
                          ))}
                        </select>
                      ) : field.type === 'number' ? (
                        <input
                          id={id}
                          class="settings-input"
                          type="number"
                          min={field.min ?? undefined}
                          max={field.max ?? undefined}
                          value={draft[field.key] === null ? '' : String(draft[field.key] ?? '')}
                          onInput={(event) => {
                            const raw = (event.currentTarget as HTMLInputElement).value;
                            const parsed = raw === '' ? Number.NaN : Number(raw);
                            setDraft((current) => ({ ...current, [field.key]: parsed }));
                          }}
                        />
                      ) : field.type === 'secret' ? (
                        <input
                          id={id}
                          class="settings-input"
                          type="password"
                          autocomplete="new-password"
                          placeholder={field.configured ? 'Saved — type to replace' : undefined}
                          value={String(draft[field.key] ?? '')}
                          onInput={(event) =>
                            setDraft((current) => ({
                              ...current,
                              [field.key]: (event.currentTarget as HTMLInputElement).value,
                            }))
                          }
                        />
                      ) : (
                        <input
                          id={id}
                          class="settings-input"
                          type="text"
                          value={String(draft[field.key] ?? '')}
                          onInput={(event) =>
                            setDraft((current) => ({
                              ...current,
                              [field.key]: (event.currentTarget as HTMLInputElement).value,
                            }))
                          }
                        />
                      )}
                    </>
                  )}
                  {field.help ? <p class="plugin-setting-help">{field.help}</p> : null}
                  {problem ? <p class="plugin-setting-error">{problem}</p> : null}
                </div>
              );
            })
          )}
          {error ? <p class="plugin-setting-error">{error}</p> : null}
          {notice ? <p class="plugin-setting-notice">{notice}</p> : null}
          <button
            type="button"
            class="button button-small"
            onClick={() => void save()}
            disabled={busy || fields === null || Object.keys(problems).length > 0}
            title="Saving restarts this plugin so it picks up the new values"
          >
            {busy ? 'Saving…' : 'Save and restart'}
          </button>
        </div>
      ) : null}
    </div>
  );
}
