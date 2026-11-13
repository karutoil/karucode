import type { JSX } from 'preact';
import { useEffect, useId, useRef, useState } from 'preact/hooks';
import type { Config } from '../api/types';
import { formatContextWindow } from '../lib/format';
import * as actions from '../state/actions';
import * as store from '../state/store';
import type { ModelSelection } from '../state/store';
import { CheckIcon, ChevronDownIcon, CpuIcon, SearchIcon, SparkIcon } from './icons';

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

/**
 * Focus one real option and keep exactly that option in the listbox tab order.
 *
 * The reasoning picker's list is short and fixed, so it keeps this walk. The
 * model picker does not: it has a filter field, and a hook that moves focus on
 * every key would pull the caret out of the field the reader is typing in.
 */
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
    // The phone sheet holds a reasoning group beside the model list. Arrow keys
    // belong to whatever the focus is on: walking the model list from an effort
    // button would move the roving tab stop behind the control being used.
    const target = event.target instanceof HTMLElement ? event.target : null;
    if (target?.getAttribute('role') !== 'option') return;
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

/**
 * Write a reasoning effort for the session's current model, with the guards the
 * picker has to apply before it writes anything.
 *
 * Shared by the wide composer's own reasoning pill and by the phone's combined
 * sheet, so both refuse the same three cases for the same reasons: a pin that is
 * still in flight (a second PATCH would race the first), a session already on
 * the endpoint default (`null` is stored as "no preference", so there is nothing
 * to clear), and a dangling pin whose profile or model no longer exists (the
 * server would reject the effort for a model it cannot resolve).
 *
 * Resolves `true` when a request was actually made.
 */
async function applyReasoningEffort(value: string): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  if (sessionId !== null && store.sessionModelMutationPending.value === sessionId) return false;
  const { providerId, modelId, pinned, effort } = effectiveSelection();
  const config = store.config.value;
  const provider = providerId ? (config?.providers.find((profile) => profile.id === providerId) ?? null) : null;
  const entry = provider?.models.find((model) => model.id === modelId);
  const dangling = Boolean(pinned && (!provider || (provider.models.length > 0 && !entry)));
  const nextEffort = value === 'default' ? null : value;
  if (nextEffort === effort) return false;
  if (nextEffort === null && !pinned) return false;
  if (dangling || !providerId || !modelId) return false;
  // The state owner's per-session generation rejects a late answer after a
  // session/project change; this component never applies that answer locally.
  await actions.setSessionModelSelection(providerId, modelId, nextEffort);
  return true;
}

/**
 * One model the picker can offer, flattened out of the provider config.
 *
 * The picker used to walk `config.providers` and draw a row per model inline,
 * which is why every row repeated the same context-window string, why a
 * single-profile install showed no provider name anywhere, and why the list
 * could not be filtered or counted. Flattening first is what lets the list be
 * searched, counted and grouped without three copies of the same traversal.
 */
export interface ModelOption {
  providerId: string;
  /** The profile's display name, falling back to its id. */
  providerName: string;
  /** Whether the profile has a key: a model on a keyless profile cannot run. */
  providerKeySet: boolean;
  modelId: string;
  /** The model's display name, falling back to its id. */
  modelName: string;
  contextWindow: number | null;
  toolCall: boolean | null;
  reasoning: boolean | null;
  vision: boolean | null;
}

/** Every model the install offers, in config order. */
export function modelOptions(config: Config | null): ModelOption[] {
  if (!config) return [];
  return config.providers.flatMap((profile) =>
    profile.models.map((model) => ({
      providerId: profile.id,
      providerName: profile.name?.trim() || profile.id,
      providerKeySet: profile.api_key_set,
      modelId: model.id,
      modelName: model.name?.trim() || model.id,
      contextWindow: model.context_window ?? null,
      toolCall: model.tool_call ?? null,
      reasoning: model.reasoning ?? null,
      vision: model.vision ?? null,
    })),
  );
}

/**
 * The models a filter query reaches.
 *
 * Every word has to appear somewhere in the model's name, its id, or its
 * provider's name or id — the same "all the words, anywhere" rule the settings
 * search uses, so a second word narrows the list instead of widening it.
 *
 * Capability words are deliberately *not* in the haystack: a model whose only
 * mention of tools is the warning "no tools" would otherwise match a search for
 * "tools", which is the opposite of what the reader asked for.
 */
export function filterModelOptions(options: readonly ModelOption[], query: string): ModelOption[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [...options];
  return options.filter((option) => {
    const haystack =
      `${option.modelName} ${option.modelId} ${option.providerName} ${option.providerId}`.toLowerCase();
    return words.every((word) => haystack.includes(word));
  });
}

/** The key a row is addressed by, in the DOM and in the roving tab stop. */
function optionKey(option: ModelOption): string {
  return `${option.providerId}:${option.modelId}`;
}

/**
 * What a row says to a screen reader: the model, where it runs, and the facts
 * the chips draw. The chips themselves are decorative — this is the sentence
 * that carries them, so the visible abbreviations ("1M ctx") never have to be
 * read out as letters.
 */
function optionLabel(option: ModelOption): string {
  const facts = [
    option.contextWindow ? `${formatContextWindow(option.contextWindow)} context` : '',
    option.toolCall === false ? 'no tool calling' : '',
    option.vision ? 'vision' : '',
    option.reasoning ? 'reasoning' : '',
  ].filter((fact) => fact !== '');
  return `${option.modelName} on ${option.providerName}${facts.length > 0 ? `, ${facts.join(', ')}` : ''}`;
}

/** Per-session model picker; opens upward from the composer. */
export function ModelSwitcher(): JSX.Element | null {
  const config = store.config.value;
  const sessionId = store.currentSessionId.value;
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [traversalKey, setTraversalKey] = useState('');
  const [pendingTarget, setPendingTarget] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const listbox = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const listboxId = useId();
  const mutationPending = sessionId !== null && store.sessionModelMutationPending.value === sessionId;
  const { providerId, modelId, pinned, effort } = effectiveSelection();
  const selectedKey = providerId && modelId ? `${providerId}:${modelId}` : '';
  const phone = store.phoneViewport.value;
  const options = modelOptions(config);
  const filtered = filterModelOptions(options, query);
  const filteredKeys = filtered.map(optionKey);
  // The roving tab stop follows the filter: a query that hides the current
  // model must not leave the list with no reachable row.
  const activeKey = filteredKeys.includes(traversalKey)
    ? traversalKey
    : filteredKeys.includes(selectedKey)
      ? selectedKey
      : (filteredKeys[0] ?? '');
  usePopover(open, () => setOpen(false), root, trigger);

  // A closed menu forgets its query: reopening it is a fresh look at the list,
  // not a continuation of a search the reader has already finished with. Doing
  // it here rather than in each close path is what makes the trigger, Escape,
  // an outside press and a selection all leave the same state behind.
  useEffect(() => {
    if (open) return;
    setQuery('');
    setTraversalKey('');
  }, [open]);

  // A desktop picker opens with the caret in the filter: the list is long enough
  // that typing a name is the fastest way through it, and the field is the one
  // place a keystroke means "narrow this". A phone does not autofocus — a
  // programmatic focus opens the on-screen keyboard, which would cover the sheet
  // the tap just opened — so it lands on the current model instead.
  useEffect(() => {
    if (!open) return;
    if (!phone) {
      search.current?.focus();
      return;
    }
    const rows = Array.from(listbox.current?.querySelectorAll<HTMLElement>('[role="option"]') ?? []);
    (rows.find((row) => row.dataset.optionKey === selectedKey) ?? rows[0])?.focus();
  }, [open, phone, selectedKey]);

  // A long list opens at the top; the model this session is on may be far down
  // it. Scrolling the row into view is what makes the checkmark findable.
  useEffect(() => {
    if (!open) return;
    const rows = Array.from(listbox.current?.querySelectorAll<HTMLElement>('[role="option"]') ?? []);
    rows.find((row) => row.dataset.optionKey === selectedKey)?.scrollIntoView({ block: 'nearest' });
  }, [open, selectedKey]);

  // A session is open - or a draft, which is a conversation that has not been
  // created yet: the pills stay mounted, and a choice made here travels in the
  // create request so the first message already runs on it.
  if (!config || (sessionId === null && store.draftSession.value === null) || config.providers.length === 0) {
    return null;
  }

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

  // A phone has room for one control in this row, so the model and the reasoning
  // effort share one pill and one sheet: the effort is a line under the model
  // name (where a desktop shows the provider) and its choices are a group at the
  // bottom of the model list. Two 24px pills with truncated labels - "Claud…",
  // "Def…" - told the reader nothing and gave them two targets to hit.
  const reasoningAvailable = entry?.reasoning !== false;
  const currentEffort =
    REASONING_OPTIONS.find((option) => option.value === (effort ?? 'default')) ?? REASONING_OPTIONS[0]!;
  const accessibleName = phone
    ? `Model and reasoning for this session: ${label} on ${providerLabel}, reasoning ${currentEffort.label}. Open the picker`
    : `Model for this session: ${label} on ${providerLabel}. Switch model`;

  const closeAndRestoreFocus = (): void => {
    setOpen(false);
    trigger.current?.focus();
  };

  const choose = async (option: ModelOption): Promise<void> => {
    if (mutationPending) return;
    closeAndRestoreFocus();
    if (option.providerId === providerId && option.modelId === modelId) return;
    setPendingTarget(optionKey(option));
    try {
      // The pinned effort (when there is one) carries over to the new model.
      await actions.setSessionModelSelection(option.providerId, option.modelId, effort);
    } finally {
      setPendingTarget(null);
    }
  };

  const chooseKey = (key: string): void => {
    const option = filtered.find((candidate) => optionKey(candidate) === key);
    if (option) void choose(option);
  };

  const chooseEffort = async (value: string): Promise<void> => {
    if (mutationPending || dangling) return;
    closeAndRestoreFocus();
    await applyReasoningEffort(value);
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

  /** Move the roving tab stop and the real focus onto one row. */
  const focusOption = (key: string): void => {
    setTraversalKey(key);
    const rows = Array.from(listbox.current?.querySelectorAll<HTMLElement>('[role="option"]') ?? []);
    rows.find((row) => row.dataset.optionKey === key)?.focus();
  };

  /**
   * Arrow keys walk the filtered list from the filter field or from a row.
   *
   * The field is where a desktop reader starts, so ArrowDown has to cross from
   * the field into the list; from a row the walk stays inside the list. Home and
   * End jump to the ends of the list that is on screen — the filtered one — so
   * they never land on a row the query hid.
   */
  const onMenuKeyDown = (event: KeyboardEvent): void => {
    const target = event.target instanceof HTMLElement ? event.target : null;
    const onRow = target?.getAttribute('role') === 'option';
    const onFilter = target === search.current;
    if (!onRow && !onFilter) return;

    if (event.key === 'Escape') {
      // A query is a thing to clear before the menu is a thing to close. The
      // popover's own Escape listener is on `document`, so stopping here is what
      // keeps the first Escape from taking the whole list away.
      if (query !== '') {
        event.preventDefault();
        event.stopPropagation();
        setQuery('');
        setTraversalKey('');
        search.current?.focus();
      }
      return;
    }

    if (event.key === 'Enter' && onFilter) {
      // Enter in the field takes the first match, the way a search box does.
      event.preventDefault();
      const first = filteredKeys[0];
      if (first) chooseKey(first);
      return;
    }

    const step = event.key === 'ArrowDown' ? 1 : event.key === 'ArrowUp' ? -1 : 0;
    if (step === 0 && event.key !== 'Home' && event.key !== 'End') return;
    if (filteredKeys.length === 0) return;
    event.preventDefault();
    const current = Math.max(0, filteredKeys.indexOf(activeKey));
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? filteredKeys.length - 1
          : (current + step + filteredKeys.length) % filteredKeys.length;
    focusOption(filteredKeys[next] ?? filteredKeys[0]!);
  };

  // Grouped by provider, in config order: a model id alone does not say which
  // endpoint will answer, and two profiles can offer the same model name.
  const groups: { providerId: string; providerName: string; providerKeySet: boolean; models: ModelOption[] }[] = [];
  for (const option of filtered) {
    const group = groups.find((candidate) => candidate.providerId === option.providerId);
    if (group) group.models.push(option);
    else
      groups.push({
        providerId: option.providerId,
        providerName: option.providerName,
        providerKeySet: option.providerKeySet,
        models: [option],
      });
  }

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
        aria-label={accessibleName}
      >
        <span class="model-switcher-text">
          <span class="model-switcher-line">
            <CpuIcon size={13} />
            <span class="model-switcher-model">{label}</span>
          </span>
          <span class="model-switcher-provider">
            {providerLabel}
            {pinSuffix}
          </span>
          {phone && reasoningAvailable ? (
            <span class="model-switcher-effort" title={`Reasoning effort: ${currentEffort.description}`}>
              <SparkIcon size={10} />
              {currentEffort.label}
            </span>
          ) : null}
        </span>
        <ChevronDownIcon size={12} class="down" />
      </button>

      {open ? (
        <div
          class="model-switcher-menu model-switcher-menu-models dropup"
          aria-label={
            phone
              ? 'Choose a model and reasoning effort for this session'
              : 'Choose a model for this session'
          }
          onKeyDown={onMenuKeyDown}
        >
          <label class="model-switcher-search">
            <SearchIcon size={13} />
            <input
              ref={search}
              type="search"
              class="model-switcher-filter"
              placeholder="Filter models"
              aria-label="Filter models"
              aria-controls={filtered.length > 0 ? listboxId : undefined}
              autocomplete="off"
              spellcheck={false}
              value={query}
              onInput={(event) => {
                setQuery(event.currentTarget.value);
                // A new query is a new list: the tab stop starts at its first row.
                setTraversalKey('');
              }}
            />
            <span class="model-switcher-count" aria-live="polite">
              {query.trim() === ''
                ? `${options.length} ${options.length === 1 ? 'model' : 'models'}`
                : `${filtered.length} of ${options.length}`}
            </span>
          </label>

          <div class="model-switcher-scroll">
            {options.length === 0 ? (
              // A fresh install with no models is not a filter that matched
              // nothing: there is no query to clear, and the fix is the footer's
              // own door into Settings.
              <p class="model-switcher-empty" role="status">
                No models configured yet. Add one under <strong>Models &amp; providers</strong>.
              </p>
            ) : filtered.length === 0 ? (
              <p class="model-switcher-empty" role="status">
                No model matches “{query.trim()}”.
                <button
                  type="button"
                  class="link-button"
                  onClick={() => {
                    setQuery('');
                    setTraversalKey('');
                    search.current?.focus();
                  }}
                >
                  Clear filter
                </button>
              </p>
            ) : (
              <div
                ref={listbox}
                id={listboxId}
                class="model-switcher-list"
                role="listbox"
                aria-label={
                  phone
                    ? 'Choose a model and reasoning effort for this session'
                    : 'Choose a model for this session'
                }
              >
                {groups.map((group) => (
                  <div
                    class="model-switcher-group"
                    key={group.providerId}
                    role="group"
                    aria-label={`${group.providerName}${group.providerKeySet ? '' : ', no API key'}`}
                  >
                    <p class="model-switcher-group-title">
                      {group.providerName}
                      {group.providerKeySet ? null : <span class="model-switcher-warn">no key</span>}
                    </p>
                    {group.models.map((option) => {
                      const key = optionKey(option);
                      const active = option.providerId === providerId && option.modelId === modelId;
                      const busy = mutationPending && pendingTarget === key;
                      return (
                        <button
                          type="button"
                          role="option"
                          aria-selected={active}
                          aria-label={optionLabel(option)}
                          class={`model-switcher-option${active ? ' is-active' : ''}`}
                          key={key}
                          data-option-key={key}
                          tabIndex={activeKey === key ? 0 : -1}
                          disabled={mutationPending}
                          onClick={() => void choose(option)}
                        >
                          <span class="model-switcher-option-main">
                            <span class="model-switcher-option-name">{option.modelName}</span>
                            <code class="model-switcher-option-id">{option.modelId}</code>
                          </span>
                          <span class="model-switcher-option-meta">
                            {busy ? (
                              'Switching…'
                            ) : (
                              <span class="model-switcher-chips" aria-hidden="true">
                                {option.contextWindow ? (
                                  <span
                                    class="model-chip"
                                    title={`${option.contextWindow.toLocaleString()} tokens of context`}
                                  >
                                    {formatContextWindow(option.contextWindow)} ctx
                                  </span>
                                ) : null}
                                {option.toolCall === false ? (
                                  <span class="model-chip model-chip-warn">no tools</span>
                                ) : null}
                                {option.vision ? <span class="model-chip">vision</span> : null}
                                {option.reasoning ? <span class="model-chip">reasoning</span> : null}
                                {active ? <CheckIcon size={13} /> : null}
                              </span>
                            )}
                          </span>
                        </button>
                      );
                    })}
                  </div>
                ))}
              </div>
            )}

            {phone && reasoningAvailable ? (
            // The same choices the standalone pill offers, in the same order, on
            // the sheet the model list already opened. `aria-pressed` rather than
            // `role="option"`: this is a group of toggles, not a second listbox,
            // so the model list's arrow-key walk never moves the reader's focus.
            //
            // It scrolls with the list rather than sitting under it: the model
            // rows are what the sheet is for, and a pinned effort block would
            // take a third of a 42dvh sheet away from them.
            <div
              class="model-switcher-group model-switcher-reasoning"
              role="group"
              aria-label="Reasoning effort for this session"
            >
              <p class="model-switcher-group-title">
                Reasoning
                {dangling ? <span class="model-switcher-warn">unavailable</span> : null}
              </p>
              <div class="reasoning-choices">
                {REASONING_OPTIONS.map((option) => {
                  const active = option.value === (effort ?? 'default');
                  return (
                    <button
                      key={option.value}
                      type="button"
                      class="reasoning-choice"
                      aria-pressed={active}
                      data-active={active ? 'true' : 'false'}
                      disabled={mutationPending || dangling}
                      title={option.description}
                      onClick={() => void chooseEffort(option.value)}
                    >
                      {option.label}
                    </button>
                  );
                })}
              </div>
            </div>
            ) : null}
          </div>

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

  // Same gate as the model picker: a session (or a draft) is open, so the picker
  // is there. On a phone the model pill *is* the reasoning picker (its sheet
  // carries the same options), because two pills do not fit in that row.
  if (
    store.phoneViewport.value ||
    !config ||
    (sessionId === null && store.draftSession.value === null) ||
    config.providers.length === 0
  ) {
    return null;
  }

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
    await applyReasoningEffort(value);
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
  if (
    !config ||
    (store.currentSessionId.value === null && store.draftSession.value === null) ||
    config.providers.length === 0
  ) {
    return null;
  }
  return (
    <div class="composer-controls">
      <ModelSwitcher />
      <ReasoningSelector />
    </div>
  );
}
