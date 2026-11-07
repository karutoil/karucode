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

/**
 * One entry of {@link REASONING_OPTIONS}: the wire value, the word the picker
 * shows, and the sentence that explains the wire value.
 */
export interface ReasoningOption {
  /**
   * What goes in `PATCH /api/sessions/:id/model` as `reasoning_effort`.
   *
   * `default` is not a server value: it means "send nothing, let the endpoint
   * decide", which `ReasoningSelector` stores as `null` when pinned.
   */
  value: string;
  /** The word in the button and in the option list. */
  label: string;
  /** The one-line explanation under the label in the option list. */
  description: string;
}

/**
 * The reasoning efforts the UI offers, in display order.
 *
 * This is the single source of truth for that list: the composer's
 * {@link ReasoningSelector} renders it, and any other surface that lets a user
 * choose an effort (the control plane's launch box) imports it rather than
 * keeping a copy, so a new effort cannot be added to one picker and not the
 * other. `REASONING_OPTIONS[0]` is the "no preference" entry.
 *
 * The server validates the value at run time — an endpoint that does not offer
 * `max` refuses it — so the list is deliberately the full set the server
 * accepts, not a per-model subset.
 */
export const REASONING_OPTIONS: readonly ReasoningOption[] = [
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
function usePopover(
  open: boolean,
  close: () => void,
  root: { current: HTMLElement | null },
  trigger: { current: HTMLButtonElement | null },
): void {
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
        event.preventDefault();
        event.stopPropagation();
        close();
        trigger.current?.focus();
      }
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open, close, root, trigger]);
}

/** Focus one real option and keep exactly that option in the listbox tab order. */
function useRovingOptions(
  open: boolean,
  selectedKey: string,
  keys: readonly string[],
  listbox: { current: HTMLElement | null },
): { activeKey: string; onKeyDown: (event: KeyboardEvent) => void } {
  const [traversalKey, setTraversalKey] = useState(selectedKey);
  const signature = keys.join('\0');
  const fallback = keys.includes(selectedKey) ? selectedKey : (keys[0] ?? '');
  const activeKey = keys.includes(traversalKey) ? traversalKey : fallback;

  useEffect(() => {
    if (!open) setTraversalKey(fallback);
  }, [open, fallback]);

  useEffect(() => {
    if (!open) return;
    const option = Array.from(listbox.current?.querySelectorAll<HTMLElement>('[role="option"]') ?? [])
      .find((candidate) => candidate.dataset.optionKey === activeKey);
    option?.focus();
  }, [open, activeKey, signature, listbox]);

  const onKeyDown = (event: KeyboardEvent): void => {
    const step = event.key === 'ArrowDown' ? 1 : event.key === 'ArrowUp' ? -1 : 0;
    if (step === 0 && event.key !== 'Home' && event.key !== 'End') return;
    const options = keys.filter((key) => {
      const element = Array.from(listbox.current?.querySelectorAll<HTMLElement>('[role="option"]') ?? [])
        .find((candidate) => candidate.dataset.optionKey === key);
      return element && !element.hasAttribute('disabled');
    });
    if (options.length === 0) return;
    event.preventDefault();
    const current = Math.max(0, options.indexOf(activeKey));
    const next = event.key === 'Home'
      ? 0
      : event.key === 'End'
        ? options.length - 1
        : (current + step + options.length) % options.length;
    setTraversalKey(options[next] ?? options[0] ?? '');
  };

  return { activeKey, onKeyDown };
}

/** Per-session model picker; opens upward from the composer. */
export function ModelSwitcher(): JSX.Element | null {
  const config = store.config.value;
  const sessionId = store.currentSessionId.value;
  const [open, setOpen] = useState(false);
  const [pendingTarget, setPendingTarget] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const listbox = useRef<HTMLDivElement>(null);
  const mutationPending = sessionId !== null && store.sessionModelMutationPending.value === sessionId;
  const { providerId, modelId, pinned, effort } = effectiveSelection();
  const selectedKey = providerId && modelId ? `${providerId}:${modelId}` : '';
  const optionKeys = (config?.providers ?? []).flatMap((profile) =>
    profile.models.map((model) => `${profile.id}:${model.id}`),
  );
  const roving = useRovingOptions(open, selectedKey, optionKeys, listbox);
  usePopover(open, () => setOpen(false), root, trigger);

  // A session is open: the pills stay mounted, whether or not the session
  // happens to have a row in a list right now.
  if (!config || sessionId === null || config.providers.length === 0) return null;

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

  const closeAndRestoreFocus = (): void => {
    setOpen(false);
    trigger.current?.focus();
  };

  const choose = async (nextProviderId: string, nextModelId: string): Promise<void> => {
    if (mutationPending) return;
    closeAndRestoreFocus();
    if (nextProviderId === providerId && nextModelId === modelId) return;
    setPendingTarget(`${nextProviderId}:${nextModelId}`);
    try {
      // The pinned effort (when there is one) carries over to the new model.
      await actions.setSessionModelSelection(nextProviderId, nextModelId, effort);
    } finally {
      setPendingTarget(null);
    }
  };

  const followDefault = async (): Promise<void> => {
    if (mutationPending) return;
    closeAndRestoreFocus();
    setPendingTarget('default');
    try {
      await actions.clearSessionModelSelection();
    } finally {
      setPendingTarget(null);
    }
  };

  return (
    <div class="model-switcher" ref={root}>
      <button
        ref={trigger}
        type="button"
        class="model-switcher-button"
        onClick={() => setOpen((value) => !value)}
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={mutationPending}
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
        <div
          ref={listbox}
          class="model-switcher-menu dropup"
          role="listbox"
          aria-label="Choose a model for this session"
          onKeyDown={roving.onKeyDown}
        >
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
                const optionKey = `${profile.id}:${model.id}`;
                const busy = mutationPending && pendingTarget === optionKey;
                return (
                  <button
                    type="button"
                    role="option"
                    aria-selected={active}
                    class={`model-switcher-option${active ? ' is-active' : ''}`}
                    key={optionKey}
                    data-option-key={optionKey}
                    tabIndex={roving.activeKey === optionKey ? 0 : -1}
                    disabled={mutationPending}
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
                disabled={mutationPending}
                onClick={() => void followDefault()}
              >
                {mutationPending && pendingTarget === 'default' ? 'Resetting…' : 'Follow install default'}
              </button>
            ) : null}
            <button
              type="button"
              class="link-button"
              disabled={mutationPending}
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
  const sessionId = store.currentSessionId.value;
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const listbox = useRef<HTMLDivElement>(null);
  const mutationPending = sessionId !== null && store.sessionModelMutationPending.value === sessionId;
  const { providerId, modelId, pinned, effort } = effectiveSelection();
  const DEFAULT_OPTION = REASONING_OPTIONS[0] as ReasoningOption;
  const current = REASONING_OPTIONS.find((option) => option.value === (effort ?? 'default')) ?? DEFAULT_OPTION;
  const roving = useRovingOptions(open, current.value, REASONING_OPTIONS.map((option) => option.value), listbox);
  usePopover(open, () => setOpen(false), root, trigger);

  // Same gate as the model picker: a session is open, so the picker is there.
  if (!config || sessionId === null || config.providers.length === 0) return null;

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

  const closeAndRestoreFocus = (): void => {
    setOpen(false);
    trigger.current?.focus();
  };

  const choose = async (value: string): Promise<void> => {
    if (mutationPending) return;
    closeAndRestoreFocus();
    const nextEffort = value === 'default' ? null : value;
    if (nextEffort === effort) return;
    if (nextEffort === null && !pinned) return;
    if (dangling || !providerId || !modelId) return;
    // The state owner's per-session generation rejects a late answer after a
    // session/project change; this component never applies that answer locally.
    await actions.setSessionModelSelection(providerId, modelId, nextEffort);
  };

  return (
    <div class="model-switcher reasoning-selector" ref={root}>
      <button
        ref={trigger}
        type="button"
        class="model-switcher-button reasoning-selector-button"
        onClick={() => setOpen((value) => !value)}
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={dangling || mutationPending}
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
        <div
          ref={listbox}
          class="model-switcher-menu dropup"
          role="listbox"
          aria-label="Choose a reasoning effort"
          onKeyDown={roving.onKeyDown}
        >
          {REASONING_OPTIONS.map((option) => {
            const active = option.value === current.value;
            return (
              <button
                type="button"
                role="option"
                aria-selected={active}
                class={`model-switcher-option${active ? ' is-active' : ''}`}
                key={option.value}
                data-option-key={option.value}
                tabIndex={roving.activeKey === option.value ? 0 : -1}
                disabled={mutationPending}
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
