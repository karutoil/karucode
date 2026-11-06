import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { formatCount } from '../lib/format';
import * as actions from '../state/actions';
import * as store from '../state/store';
import type { ModelSelection } from '../state/store';
import { CheckIcon, ChevronDownIcon, CpuIcon, SparkIcon } from './icons';

/**
 * Composer model + reasoning controls.
 *
 * Both selectors write the *session's* model pin (`PATCH /api/sessions/:id/model`),
 * so two sessions can run different models and efforts at the same time. A
 * session without a pin follows — and displays — the install-wide default.
 *
 * They are mounted whenever a session is open. They used to be mounted only
 * when the open session had a row in a session list, which made them blink out
 * whenever a list snapshot had not caught up yet (a fresh control chat, a
 * project-less session after a refresh). A control that comes and goes is not
 * a control, so the gate is "a session is open", not "a list knows about it".
 */

/** Reasoning efforts accepted by the server (`none` = reasoning off). */
const REASONING_OPTIONS: { value: string; label: string; description: string }[] = [
  { value: 'default', label: 'Default', description: 'Send nothing; the endpoint decides.' },
  { value: 'none', label: 'Off', description: 'reasoning_effort: none — skip reasoning.' },
  { value: 'minimal', label: 'Minimal', description: 'reasoning_effort: minimal.' },
  { value: 'low', label: 'Low', description: 'reasoning_effort: low.' },
  { value: 'medium', label: 'Medium', description: 'reasoning_effort: medium.' },
  { value: 'high', label: 'High', description: 'reasoning_effort: high.' },
  { value: 'max', label: 'Max', description: 'reasoning_effort: max — only some endpoints offer it.' },
];

/**
 * The selection a run of the current session would use right now.
 *
 * Delegates to `store.modelSelection`, which the composer's context pill reads
 * too: the model named here and the window the percentage is drawn against must
 * be the same model.
 */
function effectiveSelection(): ModelSelection {
  return store.modelSelection.value;
}

/** Close a popover on outside pointer-down or Escape. */
function usePopover(open: boolean, close: () => void, root: { current: HTMLElement | null }): void {
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target;
      if (target instanceof Node && root.current && !root.current.contains(target)) {
        close();
      }
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        close();
      }
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open, close, root]);
}

/** Per-session model picker; opens upward from the composer. */
export function ModelSwitcher(): JSX.Element | null {
  const config = store.config.value;
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  usePopover(open, () => setOpen(false), root);

  // A session is open: the pills stay mounted, whether or not the session
  // happens to have a row in a list right now.
  if (!config || store.currentSessionId.value === null || config.providers.length === 0) return null;
  const { providerId, modelId, pinned, effort } = effectiveSelection();

  const provider = providerId ? config.providers.find((profile) => profile.id === providerId) ?? null : null;
  const entry = provider?.models.find((model) => model.id === modelId);
  // No model at all is a real state (a fresh install with no provider models),
  // and it is drawn as an honest placeholder rather than an empty button.
  //
  // A pin can also go stale: the profile or the model it names was removed in
  // Settings, so the server falls back to the install default while the session
  // row still carries the pin. Saying "this session" about that is a lie, so it
  // is drawn as unavailable instead - still mounted, still repairable from here.
  const dangling = Boolean(
    pinned && (!provider || (provider.models.length > 0 && !entry)),
  );
  const resolved = Boolean(provider && modelId) && !dangling;
  const label = resolved
    ? (entry?.name?.trim() ? entry.name : (modelId as string))
    : dangling
      ? (modelId ?? '—')
      : '—';
  const providerLabel = dangling ? 'unavailable' : (provider?.name ?? providerId ?? 'No provider');
  const title = dangling
    ? `Pinned to ${providerId ?? '?'} / ${modelId ?? '?'}, which is no longer available — this session will use the install default. Switch model`
    : resolved
      ? `${providerLabel} · ${label} — switch model for this session`
      : 'No model configured yet';
  const pinSuffix = dangling ? ' · unavailable' : pinned ? ' · this session' : '';

  const choose = async (nextProviderId: string, nextModelId: string): Promise<void> => {
    if (nextProviderId === providerId && nextModelId === modelId) {
      setOpen(false);
      return;
    }
    setPending(`${nextProviderId}:${nextModelId}`);
    try {
      // The pinned effort (when there is one) carries over to the new model.
      const ok = await actions.setSessionModelSelection(nextProviderId, nextModelId, effort);
      if (ok) setOpen(false);
    } finally {
      setPending(null);
    }
  };

  const followDefault = async (): Promise<void> => {
    setPending('default');
    try {
      const ok = await actions.clearSessionModelSelection();
      if (ok) setOpen(false);
    } finally {
      setPending(null);
    }
  };

  return (
    <div class="model-switcher" ref={root}>
      <button
        type="button"
        class="model-switcher-button"
        onClick={() => setOpen((value) => !value)}
        aria-haspopup="listbox"
        aria-expanded={open}
        title={title}
        aria-label={`Model for this session: ${label} on ${providerLabel}. Switch model`}
      >
        <span class="model-switcher-text">
          <CpuIcon size={13} />
          <span class="model-switcher-model">{label}</span>
          <span class="model-switcher-provider">
            {providerLabel}
            {pinSuffix}
          </span>
        </span>
        <ChevronDownIcon size={12} class="down" />
      </button>

      {open ? (
        <div class="model-switcher-menu dropup" role="listbox" aria-label="Choose a model for this session">
          {config.providers.map((profile) => (
            <div class="model-switcher-group" key={profile.id}>
              {config.providers.length > 1 ? (
                <p class="model-switcher-group-title">
                  {profile.name}
                  {profile.api_key_set ? null : <span class="model-switcher-warn">no key</span>}
                </p>
              ) : null}
              {profile.models.length === 0 ? (
                <p class="model-switcher-empty">No models on this provider yet.</p>
              ) : null}
              {profile.models.map((model) => {
                const active = profile.id === providerId && model.id === modelId;
                const busy = pending === `${profile.id}:${model.id}`;
                return (
                  <button
                    type="button"
                    role="option"
                    aria-selected={active}
                    class={`model-switcher-option${active ? ' is-active' : ''}`}
                    key={`${profile.id}:${model.id}`}
                    disabled={busy}
                    onClick={() => void choose(profile.id, model.id)}
                  >
                    <span class="model-switcher-option-main">
                      <span class="model-switcher-option-name">{model.name?.trim() || model.id}</span>
                      {model.name?.trim() && model.name.trim() !== model.id ? (
                        <code class="model-switcher-option-id">{model.id}</code>
                      ) : null}
                    </span>
                    <span class="model-switcher-option-meta">
                      {busy ? (
                        'Switching…'
                      ) : (
                        <>
                          {model.context_window ? (
                            <span title="Context window">{formatCount(model.context_window)} ctx</span>
                          ) : null}
                          {model.tool_call === false ? <span title="No tool calling">no tools</span> : null}
                          {active ? <CheckIcon size={13} /> : null}
                        </>
                      )}
                    </span>
                  </button>
                );
              })}
            </div>
          ))}
          <div class="model-switcher-footer">
            {pinned ? (
              <button
                type="button"
                class="link-button"
                disabled={pending === 'default'}
                onClick={() => void followDefault()}
              >
                {pending === 'default' ? 'Resetting…' : 'Follow install default'}
              </button>
            ) : null}
            <button
              type="button"
              class="link-button"
              onClick={() => {
                setOpen(false);
                actions.openSettings();
              }}
            >
              Manage providers and models
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}

/** Per-session reasoning effort picker; hidden when the model cannot reason. */
export function ReasoningSelector(): JSX.Element | null {
  const config = store.config.value;
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  usePopover(open, () => setOpen(false), root);

  // Same gate as the model picker: a session is open, so the picker is there.
  if (!config || store.currentSessionId.value === null || config.providers.length === 0) return null;
  const { providerId, modelId, pinned, effort } = effectiveSelection();

  // The capability is looked up through the config, not through the session
  // row: the row may not exist, and the picker must not come and go with it.
  const provider = providerId ? config.providers.find((profile) => profile.id === providerId) ?? null : null;
  const entry = provider?.models.find((model) => model.id === modelId);
  // A pin whose profile or model was removed in Settings cannot be run, so its
  // effort is not a choice: the picker stays mounted (a control that vanishes is
  // worse) but says so and refuses to write a pin the server would reject.
  const dangling = Boolean(pinned && (!provider || (provider.models.length > 0 && !entry)));
  // models.dev's `reasoning: false` is a positive "cannot reason"; unknown
  // (null) stays selectable and the endpoint validates at run time.
  if (entry?.reasoning === false) return null;

  const DEFAULT_OPTION = REASONING_OPTIONS[0] as (typeof REASONING_OPTIONS)[number];
  const current = REASONING_OPTIONS.find((option) => option.value === (effort ?? 'default')) ?? DEFAULT_OPTION;

  const choose = async (value: string): Promise<void> => {
    const nextEffort = value === 'default' ? null : value;
    if (nextEffort === effort) {
      setOpen(false);
      return;
    }
    if (nextEffort === null && !pinned) {
      // Already following the install default with no override: nothing to do.
      setOpen(false);
      return;
    }
    if (dangling || !providerId || !modelId) {
      // No usable model to pin the effort to: the server would refuse the patch.
      setOpen(false);
      return;
    }
    setPending(true);
    try {
      const ok = await actions.setSessionModelSelection(providerId, modelId, nextEffort);
      if (ok) setOpen(false);
    } finally {
      setPending(false);
    }
  };

  return (
    <div class="model-switcher reasoning-selector" ref={root}>
      <button
        type="button"
        class="model-switcher-button reasoning-selector-button"
        onClick={() => setOpen((value) => !value)}
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={dangling}
        title={dangling
          ? 'This session is pinned to a model that is no longer available; its effort cannot be changed until you pick a model.'
          : 'Reasoning effort for this session'}
        aria-label={`Reasoning effort: ${current.label}. Change reasoning effort`}
      >
        <span class="model-switcher-text">
          <SparkIcon size={13} />
          <span class="model-switcher-model">
            {/* The word is for a wide composer; on a phone the spark icon and the
                effort name are what fit, and the full sentence stays in the
                label and the tooltip. Hiding it in CSS keeps it readable rather
                than ellipsised to "Rea…". */}
            <span class="reasoning-prefix">Reasoning: </span>
            {current.label}
          </span>
        </span>
        <ChevronDownIcon size={12} class="down" />
      </button>

      {open ? (
        <div class="model-switcher-menu dropup" role="listbox" aria-label="Choose a reasoning effort">
          {REASONING_OPTIONS.map((option) => {
            const active = option.value === current.value;
            return (
              <button
                type="button"
                role="option"
                aria-selected={active}
                class={`model-switcher-option${active ? ' is-active' : ''}`}
                key={option.value}
                disabled={pending}
                onClick={() => void choose(option.value)}
              >
                <span class="model-switcher-option-main">
                  <span class="model-switcher-option-name">{option.label}</span>
                  <code class="model-switcher-option-id">{option.description}</code>
                </span>
                <span class="model-switcher-option-meta">{active ? <CheckIcon size={13} /> : null}</span>
              </button>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

/** The composer's model row; renders nothing when no session is open. */
export function ModelControls(): JSX.Element | null {
  const config = store.config.value;
  if (!config || store.currentSessionId.value === null || config.providers.length === 0) return null;
  return (
    <div class="composer-controls">
      <ModelSwitcher />
      <ReasoningSelector />
    </div>
  );
}
