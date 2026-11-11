/**
 * The declarative UI layer: what a plugin asks the app to draw, validated.
 *
 * A plugin cannot ship markup, a stylesheet or a script — the frontend is a
 * compile-time snapshot embedded in the binary and the CSP protects the app's
 * origin. What it *can* do is declare, in its manifest, "put this button in that
 * slot", "add this palette command", "hide that built-in control", and answer
 * `ui/action` when a person activates it. The host validates the declaration at
 * handshake time; this module validates the payload **again**, because a client
 * that trusted a server payload would be one bug away from rendering whatever a
 * plugin felt like.
 *
 * Three closed sets keep that honest:
 *
 * * {@link UI_SLOT_IDS} — where a control may mount. This file is the mount
 *   side of `kc_core::plugin::UI_SLOTS`; the two lists are one contract.
 * * {@link UI_HIDE_TARGETS} — built-in elements a plugin may hide, mirroring
 *   `kc_core::plugin::UI_HIDEABLE`. Both are allowlists: nothing else can be
 *   hidden, and nothing safety-critical is on them.
 * * the icon set — an `icon` is a *name*, resolved by the app; an unknown name
 *   falls back to a neutral glyph instead of letting a plugin draw an SVG.
 *
 * Nothing here executes plugin code, builds a URL from a payload, or reaches for
 * `innerHTML`. Entities are drawn by first-party components only.
 */

/** The composer's action row, left of the model pins. */
export const UI_SLOT_COMPOSER_TOOLBAR = 'composer.toolbar';
/** The results inspector's header. */
export const UI_SLOT_INSPECTOR_HEADER = 'inspector.header';
/** The session header row above the transcript. */
export const UI_SLOT_SESSION_HEADER = 'session.header';

/** Every slot the app mounts. A button for any other slot is dropped. */
export const UI_SLOT_IDS: readonly string[] = [
  UI_SLOT_COMPOSER_TOOLBAR,
  UI_SLOT_INSPECTOR_HEADER,
  UI_SLOT_SESSION_HEADER,
];

/** The composer's paperclip / attach-files button. */
export const UI_HIDE_COMPOSER_ATTACH = 'composer.attach';
/** The results inspector's **Files** tab. */
export const UI_HIDE_INSPECTOR_TAB_WORKSPACE = 'inspector.tab.workspace';
/** The results inspector's **Git** tab. */
export const UI_HIDE_INSPECTOR_TAB_CHANGES = 'inspector.tab.changes';
/** The results inspector's **Session usage** tab. */
export const UI_HIDE_INSPECTOR_TAB_SESSION = 'inspector.tab.session';
/** The results inspector's **Agents** tab. */
export const UI_HIDE_INSPECTOR_TAB_AGENTS = 'inspector.tab.agents';
/**
 * The **Context** button in the session header.
 *
 * The wire id keeps saying `inspector.context` (a plugin that already asked to
 * hide it must keep working) but the control it names moved out of the results
 * header: Context has to stay reachable while the results panel is closed or
 * popped out, so it lives in the header the workbench always draws.
 */
export const UI_HIDE_CONTEXT_BUTTON = 'inspector.context';

/**
 * Every built-in element a plugin may hide.
 *
 * Deliberately short and deliberately cosmetic: a plugin must not be able to
 * remove the control that stops a run or answers an approval, and a client-side
 * list is the last line of defence if a server payload names something else.
 */
export const UI_HIDE_TARGETS: readonly string[] = [
  UI_HIDE_COMPOSER_ATTACH,
  UI_HIDE_INSPECTOR_TAB_WORKSPACE,
  UI_HIDE_INSPECTOR_TAB_CHANGES,
  UI_HIDE_INSPECTOR_TAB_SESSION,
  UI_HIDE_INSPECTOR_TAB_AGENTS,
  UI_HIDE_CONTEXT_BUTTON,
];

/** Cap on a label. Matches the host's `MAX_UI_LABEL_BYTES` closely enough. */
export const MAX_UI_LABEL_CHARS = 80;
/** Refuse a payload that declares absurdly many controls at once. */
export const MAX_UI_CONTRIBUTIONS = 64;

/**
 * Shortcuts the app itself owns, in canonical form.
 *
 * Mirrors `kc_core::plugin::UI_RESERVED_KEYS`. The host refuses these already; the
 * client refuses them too because this is where a keystroke would actually be
 * intercepted, and a double-bound chord is a bug nobody can explain.
 */
export const UI_RESERVED_KEYS: readonly string[] = [
  'mod+k',
  'mod+b',
  'mod+n',
  'mod+j',
  'mod+shift+t',
  'mod+enter',
];

/** Canonical modifier order, so two spellings of one chord compare equal. */
const MODIFIER_ORDER = ['mod', 'ctrl', 'alt', 'shift'] as const;

/** Named keys a binding may use after its modifiers. */
const NAMED_KEYS = ['enter', 'escape', 'space', '/', '[', ']', ',', '.', '-'];

/**
 * Normalise a `keys` spec, or return null when it is not usable.
 *
 * Same rule as the host: `+`-joined, case-insensitive, at least one of `mod` (Ctrl
 * on Windows/Linux, Cmd on macOS), `ctrl` or `alt`, then exactly one key. A bare
 * key is typing, not a shortcut, so it is refused on both sides.
 */
export function normaliseUiKeys(spec: string): string | null {
  const trimmed = spec.trim();
  if (trimmed === '' || trimmed.length > 32) return null;
  const mods: string[] = [];
  let key: string | null = null;
  for (const raw of trimmed.split('+')) {
    const part = raw.trim().toLowerCase();
    if (part === '') return null;
    if ((MODIFIER_ORDER as readonly string[]).includes(part)) {
      if (!mods.includes(part)) mods.push(part);
      continue;
    }
    if (key !== null) return null;
    const usable = (part.length === 1 && /[a-z0-9]/.test(part)) || NAMED_KEYS.includes(part);
    if (!usable) return null;
    key = part;
  }
  if (key === null) return null;
  if (!mods.some((mod) => mod === 'mod' || mod === 'ctrl' || mod === 'alt')) return null;
  mods.sort((a, b) => MODIFIER_ORDER.indexOf(a as never) - MODIFIER_ORDER.indexOf(b as never));
  return [...mods, key].join('+');
}

/** The key part of a keyboard event, spelled the way a spec spells it. */
function keyName(event: KeyboardEvent): string {
  const key = event.key.toLowerCase();
  if (key === ' ') return 'space';
  return key;
}

function isMac(): boolean {
  const platform =
    typeof navigator === 'undefined' ? '' : `${navigator.platform ?? ''} ${navigator.userAgent ?? ''}`;
  return /mac|iphone|ipad|ipod/i.test(platform);
}

/**
 * Whether a keyboard event is the chord a binding names.
 *
 * `mod` is the platform's usual modifier (Cmd on macOS, Ctrl elsewhere), which is
 * the same convention the app's own shortcuts use, so a plugin writes one spec and
 * it feels native on both.
 */
export function eventMatchesUiKeys(event: KeyboardEvent, spec: string): boolean {
  return matchesUiKeysOn(event, spec, { mac: isMac() });
}

/**
 * [`eventMatchesUiKeys`] with the platform spelled out.
 *
 * Exported so the two keyboard layouts can be tested without pretending to be a
 * different browser: `mod` follows Cmd on macOS and Control everywhere else, and
 * that difference is exactly what a shortcut has to get right.
 */
export function matchesUiKeysOn(
  event: KeyboardEvent,
  spec: string,
  platform: { mac: boolean },
): boolean {
  const parts = spec.split('+');
  const key = parts[parts.length - 1];
  const mods = new Set(parts.slice(0, -1));
  if (platform.mac) {
    // `mod` is Cmd; `ctrl` is the literal Control key, which is a different key.
    if (mods.has('mod') !== event.metaKey) return false;
    if (mods.has('ctrl') !== event.ctrlKey) return false;
  } else {
    // `mod` *is* Control, so either name accepts it — and the Windows/Cmd key
    // never counts, or a Copy chord would fire a plugin's binding on a Mac keyboard
    // plugged into a PC.
    const wantsControl = mods.has('mod') || mods.has('ctrl');
    if (wantsControl !== event.ctrlKey) return false;
    if (event.metaKey) return false;
  }
  if (mods.has('alt') !== event.altKey) return false;
  if (mods.has('shift') !== event.shiftKey) return false;
  return keyName(event) === key;
}

/**
 * The first contributed shortcut a keyboard event matches, or null.
 *
 * Order is payload order (discovery order, then manifest order) and the host has
 * already removed cross-plugin duplicates, so "first match wins" cannot make a
 * keystroke run two plugins. Extracted from the app's listener so the choice is
 * testable without mounting the whole workbench.
 */
export function matchUiKeybinding(
  bindings: readonly UiContribution[],
  event: KeyboardEvent,
): UiContribution | null {
  for (const binding of bindings) {
    if (binding.keys && eventMatchesUiKeys(event, binding.keys)) return binding;
  }
  return null;
}

/** One control a plugin contributed, after validation. */
export interface UiContribution {
  /** The plugin that owns it; also drawn as attribution on the control. */
  plugin: string;
  version: string;
  /** `button` (mounts in a slot), `command` (a palette entry) or `keybinding`. */
  kind: 'button' | 'command' | 'keybinding';
  /** Bare manifest id. */
  id: string;
  /** `plugin__<plugin>__<id>`: the DOM key and the stable identity. */
  hosted_id: string;
  /** For a button: the slot it mounts in. */
  slot: string | null;
  label: string;
  title: string | null;
  icon: string | null;
  /** The `ui/action` id to run. Exactly one of `action` / `message` is set. */
  action: string | null;
  /** A user turn to send instead of running an action. */
  message: string | null;
  /** For a keybinding: the canonical chord (`mod+shift+f`). */
  keys: string | null;
}

/** A built-in element one or more plugins asked the app to hide. */
export interface UiHiddenTarget {
  target: string;
  plugins: string[];
}

/** Everything the app derived from `GET /api/config`'s `plugin_ui`. */
export interface ParsedPluginUi {
  /** Every accepted control, in payload order (discovery order, then manifest). */
  contributions: UiContribution[];
  /** Accepted buttons grouped by slot. A slot with none is absent. */
  bySlot: Readonly<Record<string, UiContribution[]>>;
  /** Accepted palette commands, in payload order. */
  commands: UiContribution[];
  /** Accepted keyboard shortcuts, in payload order (first claim on a chord wins). */
  keybindings: UiContribution[];
  /** Hidden built-in targets, with the plugins that asked for each. */
  hidden: Readonly<Record<string, string[]>>;
  /** Slots the server says it mounts (informational, for diagnostics). */
  slots: string[];
  /** Targets the server says are hideable (informational, for diagnostics). */
  hideable: string[];
  /** Why an entry was dropped. Never rendered as UI; useful in tests and logs. */
  refused: string[];
}

const EMPTY: ParsedPluginUi = {
  contributions: [],
  bySlot: {},
  commands: [],
  keybindings: [],
  hidden: {},
  slots: [],
  hideable: [],
  refused: [],
};

/** A plugin id or contribution id: lowercase, no separators that would break a URL. */
const ID_RE = /^[a-z0-9._-]+$/;

function text(value: unknown): string {
  return typeof value === 'string' ? value.trim() : '';
}

function stringOrNull(value: unknown): string | null {
  const trimmed = text(value);
  return trimmed === '' ? null : trimmed;
}

/**
 * Validate one contribution from the wire.
 *
 * Returns the control, or a reason string. Every branch is a case a hostile or
 * stale payload can actually produce: a kind this build does not draw, a slot it
 * does not mount, an id that would break a DOM key or a URL, a control with two
 * behaviours or none.
 */
function readContribution(
  raw: unknown,
  seen: Set<string>,
  bound: Map<string, string>,
): UiContribution | string {
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) {
    return 'an entry is not an object';
  }
  const record = raw as Record<string, unknown>;
  const kind = text(record.kind);
  if (kind !== 'button' && kind !== 'command' && kind !== 'keybinding') {
    // `hide` entries never arrive here (the host unions them into `hidden`), so
    // anything else is a shape this build does not draw.
    return `kind \`${kind || '?'}\` is not drawable`;
  }
  const plugin = text(record.plugin);
  const id = text(record.id);
  if (!ID_RE.test(plugin) || !ID_RE.test(id)) {
    return `\`${plugin || '?'}\`/\`${id || '?'}\` is not a usable id`;
  }
  const hosted_id = text(record.hosted_id);
  if (hosted_id !== `plugin__${plugin}__${id}`) {
    return `hosted id \`${hosted_id || '?'}\` does not match its plugin and id`;
  }
  if (seen.has(hosted_id)) return `\`${hosted_id}\` is declared twice`;
  const label = text(record.label);
  if (label === '') return `\`${hosted_id}\` has no label`;
  if (label.length > MAX_UI_LABEL_CHARS) {
    return `\`${hosted_id}\` has a ${label.length}-character label`;
  }
  const action = stringOrNull(record.action);
  const message = stringOrNull(record.message);
  if (action !== null && message !== null) {
    return `\`${hosted_id}\` declares both an action and a message`;
  }
  if (action === null && message === null) {
    return `\`${hosted_id}\` declares neither an action nor a message`;
  }
  if (action !== null && !ID_RE.test(action)) {
    return `\`${hosted_id}\` has the unusable action id \`${action}\``;
  }
  let slot: string | null = null;
  if (kind === 'button') {
    slot = text(record.slot);
    if (!UI_SLOT_IDS.includes(slot)) {
      return `\`${hosted_id}\` targets the slot \`${slot || '?'}\`, which this build does not mount`;
    }
  }
  let keys: string | null = null;
  if (kind === 'keybinding') {
    const raw_keys = text(record.keys);
    if (raw_keys === '') return `\`${hosted_id}\` has no shortcut`;
    keys = normaliseUiKeys(raw_keys);
    if (keys === null) {
      return `\`${hosted_id}\` has the unusable shortcut \`${raw_keys}\``;
    }
    if (UI_RESERVED_KEYS.includes(keys)) {
      return `\`${hosted_id}\` binds \`${keys}\`, which is one of the app's own shortcuts`;
    }
    const owner = bound.get(keys);
    if (owner !== undefined) {
      return `\`${hosted_id}\` binds \`${keys}\`, already bound by \`${owner}\``;
    }
    bound.set(keys, hosted_id);
  }
  seen.add(hosted_id);
  return {
    plugin,
    version: text(record.version),
    kind,
    id,
    hosted_id,
    slot,
    label,
    title: stringOrNull(record.title),
    icon: stringOrNull(record.icon),
    action,
    message,
    keys,
  };
}

/**
 * Read the hidden-target list.
 *
 * A target this build does not know is ignored rather than trusted: a newer
 * server may name a control an older client draws differently, and hiding the
 * wrong thing is worse than hiding nothing.
 */
function readHidden(raw: unknown, refused: string[]): Record<string, string[]> {
  const hidden: Record<string, string[]> = {};
  if (!Array.isArray(raw)) return hidden;
  for (const entry of raw) {
    if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) continue;
    const record = entry as Record<string, unknown>;
    const target = text(record.target);
    if (!UI_HIDE_TARGETS.includes(target)) {
      if (target !== '') refused.push(`hide target \`${target}\` is not hideable`);
      continue;
    }
    const plugins = Array.isArray(record.plugins)
      ? record.plugins.map(text).filter((name) => name !== '')
      : [];
    hidden[target] = [...(hidden[target] ?? []), ...plugins];
  }
  return hidden;
}

function readStrings(raw: unknown): string[] {
  return Array.isArray(raw) ? raw.map(text).filter((v) => v !== '') : [];
}

/**
 * Turn `plugin_ui` (or an older server's absence of it) into a render model.
 *
 * Never throws: a malformed payload yields fewer controls and a `refused` reason,
 * because a plugin's mistake must not take the workbench down.
 */
export function parsePluginUi(raw: unknown): ParsedPluginUi {
  if (raw === null || raw === undefined) return EMPTY;
  if (typeof raw !== 'object' || Array.isArray(raw)) return EMPTY;
  const payload = raw as Record<string, unknown>;
  const rawContributions = Array.isArray(payload.contributions) ? payload.contributions : [];
  const refused: string[] = [];
  if (rawContributions.length > MAX_UI_CONTRIBUTIONS) {
    // Keep the first N and say so, rather than dropping the whole set.
    refused.push(`${rawContributions.length} contributions; only ${MAX_UI_CONTRIBUTIONS} are drawn`);
  }
  const seen = new Set<string>();
  const bound = new Map<string, string>();
  const contributions: UiContribution[] = [];
  for (const entry of rawContributions.slice(0, MAX_UI_CONTRIBUTIONS)) {
    const read = readContribution(entry, seen, bound);
    if (typeof read === 'string') refused.push(read);
    else contributions.push(read);
  }
  const bySlot: Record<string, UiContribution[]> = {};
  const commands: UiContribution[] = [];
  const keybindings: UiContribution[] = [];
  for (const contribution of contributions) {
    if (contribution.kind === 'command') {
      commands.push(contribution);
      continue;
    }
    if (contribution.kind === 'keybinding') {
      keybindings.push(contribution);
      continue;
    }
    const slot = contribution.slot;
    if (slot === null) continue;
    (bySlot[slot] ??= []).push(contribution);
  }
  return {
    contributions,
    bySlot,
    commands,
    keybindings,
    hidden: readHidden(payload.hidden, refused),
    slots: readStrings(payload.slots),
    hideable: readStrings(payload.hideable),
    refused,
  };
}

/** The buttons contributed to one slot, in payload order. */
export function slotContributions(ui: ParsedPluginUi, slot: string): UiContribution[] {
  return ui.bySlot[slot] ?? [];
}

/** Whether a plugin asked the app to hide one built-in control. */
export function isUiHidden(ui: ParsedPluginUi, target: string): boolean {
  return Object.prototype.hasOwnProperty.call(ui.hidden, target);
}

/**
 * The person's own overrides, kept per browser.
 *
 * A plugin's declaration is a *request*: it can add a control and it can ask for
 * one of the app's own to be hidden. This is the other side of that bargain —
 * whoever is looking at the screen can switch a contributed control off, or put a
 * hidden one back, without disabling the whole plugin. Stored locally because it
 * is a preference about *this* browser, exactly like the theme and the rail state.
 */
export interface PluginUiPreferences {
  /** Hosted ids of contributions the person switched off. */
  disabled: string[];
  /** Hide targets the person overrode: the app draws them again. */
  shown: string[];
}

/** No overrides: every plugin contribution and every hide is honoured. */
export const NO_UI_PREFERENCES: PluginUiPreferences = { disabled: [], shown: [] };

/** True when the person switched one contributed control off. */
export function isContributionDisabled(prefs: PluginUiPreferences, hostedId: string): boolean {
  return prefs.disabled.includes(hostedId);
}

/** True when the person overrode a plugin's hide, so the built-in draws again. */
export function isHideOverridden(prefs: PluginUiPreferences, target: string): boolean {
  return prefs.shown.includes(target);
}

/**
 * Apply the person's overrides to a parsed payload.
 *
 * Deliberately a pure transform of the same model: the components downstream never
 * learn whether a control is missing because no plugin declared it, because the
 * person switched it off, or because its plugin was disabled. That keeps one code
 * path and one set of layout consequences.
 *
 * The slots and hideable catalogues are passed through untouched — they describe
 * the app, not the plugins.
 */
export function applyUiPreferences(
  ui: ParsedPluginUi,
  prefs: PluginUiPreferences,
): ParsedPluginUi {
  if (prefs.disabled.length === 0 && prefs.shown.length === 0) return ui;
  const disabled = new Set(prefs.disabled);
  const shown = new Set(prefs.shown);
  const contributions = ui.contributions.filter((c) => !disabled.has(c.hosted_id));
  const bySlot: Record<string, UiContribution[]> = {};
  const commands: UiContribution[] = [];
  const keybindings: UiContribution[] = [];
  for (const contribution of contributions) {
    if (contribution.kind === 'command') {
      commands.push(contribution);
      continue;
    }
    if (contribution.kind === 'keybinding') {
      keybindings.push(contribution);
      continue;
    }
    if (contribution.slot === null) continue;
    (bySlot[contribution.slot] ??= []).push(contribution);
  }
  const hidden: Record<string, string[]> = {};
  for (const [target, plugins] of Object.entries(ui.hidden)) {
    if (!shown.has(target)) hidden[target] = plugins;
  }
  return { ...ui, contributions, bySlot, commands, keybindings, hidden };
}

/**
 * The plugins that asked to hide a control, for the tooltip that explains a
 * missing button. Empty when nothing hid it.
 */
export function uiHiddenBy(ui: ParsedPluginUi, target: string): string[] {
  return ui.hidden[target] ?? [];
}

/**
 * Filter built-in tabs by the hide list, never leaving the strip empty.
 *
 * Hiding is a cosmetic request, not a way to take the inspector's last tab: when
 * every built-in tab would be hidden, the list comes back unfiltered and the
 * refusal is reported, so a plugin cannot leave somebody with an empty panel.
 */
export function visibleTabs<T extends { id: string }>(
  tabs: readonly T[],
  ui: ParsedPluginUi,
  targetFor: (id: string) => string | null,
): { tabs: T[]; refused: boolean } {
  const kept = tabs.filter((tab) => {
    const target = targetFor(tab.id);
    return target === null || !isUiHidden(ui, target);
  });
  if (kept.length === 0 && tabs.length > 0) {
    return { tabs: [...tabs], refused: true };
  }
  return { tabs: kept, refused: false };
}
