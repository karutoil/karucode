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

import type { PluginUiContributionWire, PluginUiHiddenWire } from '../api/types';

/** The composer's action row, left of the model pins. */
export const UI_SLOT_COMPOSER_TOOLBAR = 'composer.toolbar';
/** The results inspector's header. */
export const UI_SLOT_INSPECTOR_HEADER = 'inspector.header';
/** The session header row above the transcript. */
export const UI_SLOT_SESSION_HEADER = 'session.header';
/** A chip on one row of the project rail's session list. */
export const UI_SLOT_SESSION_ROW_BADGE = 'session.row.badge';
/** A chip beside the open session's title. */
export const UI_SLOT_SESSION_HEADER_CHIP = 'session.header.chip';

/**
 * Every slot the app mounts. A button for any other slot is dropped.
 *
 * This list and the mount sites are kept in step on purpose: a slot declared
 * here but rendered nowhere is worse than no slot at all, because the host
 * validates the declaration and the plugin author sees success while nothing
 * appears. Two entries were removed for exactly that reason: `session.row.menu`
 * (a menu entry needs the rail's own `MenuItem` rendering, which a contributed
 * button or chip cannot provide) and `statusbar.item` (this build has no
 * application status bar — only the file viewer's, which is not a plugin
 * surface).
 */
export const UI_SLOT_IDS: readonly string[] = [
  UI_SLOT_COMPOSER_TOOLBAR,
  UI_SLOT_INSPECTOR_HEADER,
  UI_SLOT_SESSION_HEADER,
  UI_SLOT_SESSION_ROW_BADGE,
  UI_SLOT_SESSION_HEADER_CHIP,
];

/**
 * Slots whose entry is a **chip** rather than a labelled control.
 *
 * A chip shows `text` and a `tone`; everything else about it is a button (it
 * carries one `action` or one `message`). Kept here as one list so the renderer
 * and the payload validation cannot disagree.
 */
export const UI_BADGE_SLOTS: readonly string[] = [
  UI_SLOT_SESSION_ROW_BADGE,
  UI_SLOT_SESSION_HEADER_CHIP,
];

/** The tones a chip may use. Mirrors `kc_core::plugin::UI_TONES`. */
export const UI_TONES: readonly string[] = ['neutral', 'accent', 'success', 'warn', 'danger', 'muted'];
/** The tone drawn when a chip names none, or names one this build does not have. */
export const UI_TONE_DEFAULT = 'neutral';

/** The composer's paperclip / attach-files button. */
export const UI_HIDE_COMPOSER_ATTACH = 'composer.attach';
/** The composer's model / provider picker. */
export const UI_HIDE_COMPOSER_MODEL = 'composer.model';
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
/** The rail's footer (theme and settings buttons). */
export const UI_HIDE_RAIL_FOOTER = 'rail.footer';
/** The session header's action buttons, beside the title. */
export const UI_HIDE_SESSION_HEADER_ACTIONS = 'session.header.actions';
/** The notification bell and its panel. */
export const UI_HIDE_NOTIFICATIONS = 'notifications';
/** The visible command-palette affordance (the chord stays bound). */
export const UI_HIDE_COMMAND_PALETTE = 'command-palette';

/**
 * Every built-in element a plugin may hide.
 *
 * Deliberately short and deliberately cosmetic: a plugin must not be able to
 * remove the control that stops a run or answers an approval, and a client-side
 * list is the last line of defence if a server payload names something else.
 *
 * Every entry here is consumed by an `isUiHidden(...)` call at its real control;
 * `statusbar` was removed because this build has no application status bar to
 * hide, and a target that nothing reads is a promise the app does not keep.
 *
 * No combination of hides can lock somebody out of Settings: `rail.footer` (the
 * settings avatar) is the one control that opens Settings directly, so hiding it
 * always keeps the bell (when the bell was hidden too) or the command palette —
 * *Settings* is one of its commands. The rule is implemented once, in
 * {@link settingsHides}, and every mount site reads the result instead of the
 * raw flag.
 */
export const UI_HIDE_TARGETS: readonly string[] = [
  UI_HIDE_COMPOSER_ATTACH,
  UI_HIDE_COMPOSER_MODEL,
  UI_HIDE_INSPECTOR_TAB_WORKSPACE,
  UI_HIDE_INSPECTOR_TAB_CHANGES,
  UI_HIDE_INSPECTOR_TAB_SESSION,
  UI_HIDE_INSPECTOR_TAB_AGENTS,
  UI_HIDE_CONTEXT_BUTTON,
  UI_HIDE_RAIL_FOOTER,
  UI_HIDE_SESSION_HEADER_ACTIONS,
  UI_HIDE_NOTIFICATIONS,
  UI_HIDE_COMMAND_PALETTE,
];

/** The product name in the rail's lockup. */
export const UI_REPLACE_APP_NAME = 'app.name';
/**
 * The word for a conversation ("Session").
 *
 * Reserved, not reachable: no control in this build draws one word for
 * "session", so this is deliberately absent from `UI_REPLACE_TARGETS` and a
 * `replace` naming it is refused. The constant stays so the spelling is fixed
 * for the day a single control does.
 */
export const UI_REPLACE_SESSION_LABEL = 'label.session';
/**
 * The word for a workspace root ("Project").
 *
 * Reserved, not reachable for the same reason as `label.session`: the rail, the
 * project sheet and the dialogs each phrase their own copy, so there is no one
 * label a rename could change.
 */
export const UI_REPLACE_PROJECT_LABEL = 'label.project';
/** The "New session" button label. */
export const UI_REPLACE_NEW_SESSION_LABEL = 'label.new-session';
/** The empty-state greeting above the composer. */
export const UI_REPLACE_GREETING = 'empty.greeting';

/**
 * Every element a plugin may **replace the text of**, rather than hide.
 *
 * Mirrors `kc_core::plugin::UI_REPLACEABLE`. Nothing that states a safety
 * property is on it: a plugin may rename "New session" to "New thread", never
 * relabel an approval control or rewrite the unsandboxed-shell warning.
 *
 * Every entry here is consumed by a `pluginText(...)` call at the control it
 * names — the rail lockup (`app.name`, `ProjectRail.tsx`), the "New session"
 * buttons (`label.new-session`, `SessionsRail.tsx`) and the empty-state heading
 * (`empty.greeting`, `EmptyState.tsx`). `label.session` and `label.project` were
 * dropped from the catalogue because nothing rendered them, and a target that
 * nothing reads is a promise the app does not keep. `uiContributions.test.ts`
 * asserts the two lists agree, so the client and the host cannot drift apart.
 */
export const UI_REPLACE_TARGETS: readonly string[] = [
  UI_REPLACE_APP_NAME,
  UI_REPLACE_NEW_SESSION_LABEL,
  UI_REPLACE_GREETING,
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
  /** The accessible name; for a chip, the name a screen reader hears. */
  label: string;
  title: string | null;
  icon: string | null;
  /** The `ui/action` id to run. Exactly one of `action` / `message` is set. */
  action: string | null;
  /** A user turn to send instead of running an action. */
  message: string | null;
  /** For a keybinding: the canonical chord (`mod+shift+f`). */
  keys: string | null;
  /**
   * For a chip in a {@link UI_BADGE_SLOTS} slot: the short text drawn on it.
   *
   * A chip is text plus a tone rather than a labelled control, so this — not
   * `label` — is what a person reads. Null on every other kind.
   */
  text: string | null;
  /** For a chip: one of {@link UI_TONES}; never null, `neutral` when unstated. */
  tone: string;
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
  /**
   * Renamed built-in labels: target → the text to draw instead.
   *
   * Asked for by a `replace` contribution. Consumers call `pluginText(ui, target,
   * fallback)` rather than reading this directly, so there is one place that
   * decides what a missing or empty replacement means.
   */
  replacements: Readonly<Record<string, string>>;
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
  replacements: {},
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
 * The tone a chip is drawn with: its own when this build knows it, `neutral`
 * otherwise.
 *
 * The host already resolved an unknown tone to `neutral`, so this only matters
 * for a stale or hand-written payload — but drawing a tone the stylesheet has no
 * class for would silently drop the chip's only visual signal.
 */
function chipTone(value: unknown): string {
  const trimmed = text(value);
  return UI_TONES.includes(trimmed) ? trimmed : UI_TONE_DEFAULT;
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
  if (kind === 'replace') {
    // A `replace` is not a control: it renames a built-in label and is collected
    // by `readReplacements`. Reaching here only means it is skipped as a control.
    return `kind \`replace\` is not a drawable control`;
  }
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
  // A chip in a badge slot is drawn from `text`, and `text` is its content: the
  // label is only the accessible name (the host fills it in from `text` when the
  // plugin gave none). Refusing a chip for an empty label would throw away the one
  // thing it exists to show, so the cap below applies to whichever string is drawn.
  const is_chip = slot !== null && UI_BADGE_SLOTS.includes(slot);
  const chip_text = stringOrNull(record.text);
  if (label === '' && !is_chip) return `\`${hosted_id}\` has no label`;
  if (label.length > MAX_UI_LABEL_CHARS) {
    return `\`${hosted_id}\` has a ${label.length}-character label`;
  }
  if (is_chip && label === '' && chip_text === null) {
    return `\`${hosted_id}\` has neither a text nor a label to draw`;
  }
  if (chip_text !== null && chip_text.length > MAX_UI_LABEL_CHARS) {
    return `\`${hosted_id}\` has ${chip_text.length} characters of chip text`;
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
    text: chip_text,
    tone: chipTone(record.tone),
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
    // Typed as the wire shape rather than cast to a bare record: the field names
    // this reads are then the ones `PluginUiHiddenWire` declares, so a server that
    // renames one fails to compile here instead of silently hiding nothing.
    const record = entry as PluginUiHiddenWire | null;
    if (record === null || typeof record !== 'object' || Array.isArray(record)) continue;
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
 * Read the `replace` contributions into a target → text map.
 *
 * The host already refused any target outside the replaceable catalogue and any
 * empty or over-long text, so this is defence in depth: a target this build does
 * not know is dropped with a reason, and the *first* claim on a target wins in
 * payload (discovery) order, matching the deterministic resolution every other
 * contribution uses.
 */
function readReplacements(raw: unknown, refused: string[]): Record<string, string> {
  const replacements: Record<string, string> = {};
  if (!Array.isArray(raw)) return replacements;
  for (const entry of raw) {
    // Same reasoning as `readHidden`: the type is the wire record, so `kind`,
    // `target` and `replace` are the names `PluginUiContributionWire` declares.
    const record = entry as PluginUiContributionWire | null;
    if (record === null || typeof record !== 'object' || Array.isArray(record)) continue;
    if (text(record.kind) !== 'replace') continue;
    const target = text(record.target);
    const label = text(record.replace);
    if (!UI_REPLACE_TARGETS.includes(target)) {
      if (target !== '') refused.push(`replace target \`${target}\` is not replaceable`);
      continue;
    }
    if (label === '') {
      refused.push(`replace for \`${target}\` has no text`);
      continue;
    }
    if (label.length > MAX_UI_LABEL_CHARS) {
      refused.push(`replace for \`${target}\` is ${label.length} characters`);
      continue;
    }
    if (Object.prototype.hasOwnProperty.call(replacements, target)) {
      refused.push(`replace target \`${target}\` is renamed twice; the first wins`);
      continue;
    }
    replacements[target] = label;
  }
  return replacements;
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
    replacements: readReplacements(rawContributions, refused),
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
 * The hides that actually apply, once the escape hatch is resolved.
 *
 * Hiding is cosmetic, but `rail.footer` (the settings avatar), `command-palette`
 * and `notifications` are the three entries into Settings — and Settings is where
 * the person puts a hidden control back. Hiding all three would therefore be
 * irreversible from inside the app: no avatar, no bell, no palette to run
 * *Settings* from. So the app keeps a way out instead of honouring the last one:
 *
 * * the palette is not an entry by itself — it is a way to *run* the Settings
 *   command, which needs somebody to open it first. The bell is not an entry
 *   either, for the same reason. The avatar is the only control that opens
 *   Settings directly. So a hide of `rail.footer` always keeps one of the other
 *   two: the bell when the payload asked for the bell (it is the smaller control,
 *   and the person may have asked for exactly the avatar-plus-bell pair), or the
 *   palette when it did not — *Settings* is one of its commands, so that is the
 *   escape hatch out of the state where the avatar is gone.
 *
 * One hide is overridden at most, and only when `rail.footer` was hidden: the
 * bell when the bell was hidden as well, otherwise nothing (`rail.footer` alone
 * leaves the palette on screen, and no hide was refused).
 *
 * A single function so the rule is decided once. No component special-cases the
 * pair on its own: they read this instead of the raw flag, and the settings
 * screen reports the override (see {@link settingsEscapeHatch}) so a plugin author
 * can see that one of its hides did not take effect.
 *
 * The rule is a property of the hide list, not a patch of the previous one: the
 * case that keeps anything is "`rail.footer` is hidden", and the hide that is
 * dropped is never the footer. Running it again therefore changes nothing — and
 * says nothing, because {@link settingsEscapeHatch} reports a kept entry only when
 * the payload asked for it. The store applies the person's overrides to the
 * *declared* payload and this to that result, so the two passes must agree.
 *
 * Returning the `ParsedPluginUi` (rather than three booleans) keeps
 * `ui.hidden` the only truth: a caller that wants `isUiHidden` still gets it.
 */
export function settingsHides(ui: ParsedPluginUi): ParsedPluginUi {
  // Decided from what was *asked*, never from a previously resolved map: a rule
  // that re-read its own output would be a two-state loop, because keeping the
  // bell is exactly the thing that would then justify dropping the palette.
  const requested = ui.hidden;
  const footer = Object.prototype.hasOwnProperty.call(requested, UI_HIDE_RAIL_FOOTER);
  const askedBell = Object.prototype.hasOwnProperty.call(requested, UI_HIDE_NOTIFICATIONS);
  // The avatar is the only control that opens Settings directly, so a hide of it
  // always keeps one of the two: the bell when it was asked for (and is the
  // smaller control), otherwise the palette, whose *Settings* command is the
  // escape hatch out of the state where the avatar is gone.
  const keepBell = askedBell && footer;
  const keepPalette = !askedBell && footer;
  if (!keepBell && !keepPalette) return ui;
  const hidden = { ...ui.hidden };
  if (keepBell) delete hidden[UI_HIDE_NOTIFICATIONS];
  if (keepPalette) delete hidden[UI_HIDE_COMMAND_PALETTE];
  return { ...ui, hidden };
}

/**
 * The hide target the app kept alive against a plugin's request, or `null`.
 *
 * One entry point at most can be overridden, so a single answer is enough for the
 * settings screen and for the diagnostics line the person reads. Null means every
 * hide took effect (or nothing asked for the combination that would lock the app).
 */
export function settingsEscapeHatch(ui: ParsedPluginUi): string | null {
  const resolved = settingsHides(ui);
  // `settingsHides` builds a fresh `{ ...ui, hidden }` whenever it keeps anything,
  // so identity can never be the "nothing changed" signal: ask the resolved hide
  // map. `isUiHidden` is the right question here — not "is the target mentioned
  // anywhere in the payload", which a plugin that hid the rail footer *and* the
  // bell would also satisfy.
  if (isUiHidden(ui, UI_HIDE_NOTIFICATIONS) && !isUiHidden(resolved, UI_HIDE_NOTIFICATIONS)) {
    return UI_HIDE_NOTIFICATIONS;
  }
  if (isUiHidden(ui, UI_HIDE_COMMAND_PALETTE) && !isUiHidden(resolved, UI_HIDE_COMMAND_PALETTE)) {
    return UI_HIDE_COMMAND_PALETTE;
  }
  return null;
}

/**
 * The text to draw for a built-in label: a plugin's replacement when it asked
 * for one, the app's own word otherwise.
 *
 * One function rather than reading `ui.replacements` at each call site, so
 * "replaced", "never asked" and "asked with an empty string" can never diverge.
 * Nothing on the replaceable catalogue states a safety property, so a rename can
 * never change what an approval control or the sandbox warning says.
 */
export function pluginText(ui: ParsedPluginUi, target: string, fallback: string): string {
  const replacement = ui.replacements[target];
  return replacement !== undefined && replacement !== '' ? replacement : fallback;
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
