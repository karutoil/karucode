import type { JsonValue } from './common';

export interface ConfigPluginSummary {
  name: string;
  version: string;
  status: string;
  tools: string[];
  error: string | null;
  permissions: string[];
}

/**
 * One inspector panel a plugin contributes.
 *
 * `data` is not part of this: it is fetched per panel from
 * `GET /api/plugins/:name/panels/:panel` and is opaque JSON, so it is typed as
 * `unknown` at that boundary rather than here.
 */
export interface PluginPanelSummary {
  id: string;
  /** `plugin__<plugin>__<id>`: the tab id and the key for a renderer. */
  hosted_id: string;
  title: string;
  icon: string | null;
  /** `tool` or `static`. Both are read through `panels/read`. */
  source: string;
  /** Suggested poll interval in ms; 0 means "only on explicit refresh". */
  refresh_ms: number;
}

/** Panels contributed by one plugin, in manifest order. */
export interface PluginPanelGroup {
  plugin: string;
  version: string;
  panels: PluginPanelSummary[];
}

/** `GET /api/plugins/:name/panels/:panel` — the payload the UI renders. */
export interface PluginPanelData {
  /** Title the plugin wants shown (its manifest's title when it omits one). */
  title: string;
  /** Opaque JSON. Untrusted: validate before reading any field. */
  data: unknown;
  /** Cache key; changes whenever a fresh read happened. */
  version: number;
  fetched_ms: number;
}

/**
 * One control a plugin contributes, as `GET /api/config` describes it.
 *
 * Described, never executed: `kind`, `slot`, `icon` and `target` are names from
 * closed sets the app owns, and `label`/`title` are plain text. Nothing here is
 * markup, a URL, a selector or code.
 */
export interface PluginUiContributionWire {
  plugin: string;
  version: string;
  /** `button`, `command` or `keybinding`. (`hide` entries are folded into `hidden`.) */
  kind: string;
  id: string;
  hosted_id: string;
  /** For a button: the slot it mounts in. */
  slot?: string | null;
  /**
   * The accessible name. For a chip (`session.row.badge`, `session.header.chip`)
   * this is the name a screen reader hears while `text` is what is drawn.
   */
  label: string;
  title?: string | null;
  icon?: string | null;
  /** The `ui/action` id to run. Exactly one of `action` / `message` is set. */
  action?: string | null;
  /** A user turn to send instead of running an action. */
  message?: string | null;
  /** For a keybinding: the canonical chord (`mod+shift+f`). */
  keys?: string | null;
  /** For `replace`: the built-in element whose label is being renamed. */
  target?: string | null;
  /** For `replace`: the text to draw in place of the app's own label. */
  replace?: string | null;
  /** For a chip: the short text drawn on it. Absent on every other kind. */
  text?: string | null;
  /** For a chip: one of `UI_TONES`. An unknown or absent tone draws `neutral`. */
  tone?: string | null;
}

/** A built-in control one or more plugins asked the app to hide. */
export interface PluginUiHiddenWire {
  target: string;
  /** Plugins that asked, so a missing button can name who took it. */
  plugins: string[];
}

/**
 * `GET /api/config`'s `plugin_ui`.
 *
 * `slots` and `hideable` are the server's catalogues: they travel with the
 * payload so a client can report a mismatch instead of hard-coding a mount point.
 */
export interface PluginUiConfig {
  contributions?: PluginUiContributionWire[];
  hidden?: PluginUiHiddenWire[];
  slots?: string[];
  hideable?: string[];
}

/** `POST /api/plugins/:name/ui/actions/:action` — the plugin's validated answer. */
export interface PluginUiActionResult {
  /** `false` renders the message as an error. */
  ok: boolean;
  /** The toast text. Already capped by the host. */
  message: string;
  /** Bare panel ids of this plugin the UI should re-read. */
  refresh: string[];
  /** An `http`/`https` URL to open in a new tab, or null. */
  open_url: string | null;
}

export interface PluginToolSpec {
  name: string;
  description: string;
  parameters: JsonValue;
}

export interface PluginManifest {
  name: string;
  version: string;
  protocol?: string;
  capabilities?: string[];
  permissions?: string[];
  hooks?: string[];
}

export interface PluginDiagnostics {
  name: string;
  status: string;
  /** Absent when the plugin failed before its manifest was read. Read as `?.`. */
  manifest?: PluginManifest | null;
  tools: PluginToolSpec[];
  /** Absent when there is nothing wrong. Read as a truthiness test, so safe. */
  error?: string | null;
  pid: number | null;
  restart_count: number;
  /**
   * The tool-output budget that applies to this plugin's results.
   *
   * `effective_bytes` is what the runtime really truncates to: the operator's
   * grant when there is one, otherwise the global `limits.max_tool_output_bytes`.
   * `granted` says which, and `pending` carries an ask waiting for the operator.
   */
  tool_output?: PluginToolOutputSummary;
}

/**
 * The per-plugin tool-output budget, as `GET /api/plugins` reports it.
 *
 * A budget is a *record of an operator decision*: an agent asks for one with a
 * byte value and a reason, and only an answer creates it. Nothing here can be
 * set by a plugin or by editing a config key.
 */
export interface PluginToolOutputSummary {
  effective_bytes: number;
  granted: boolean;
  global_bytes: number;
  ceiling_bytes: number;
  /** The operator's `[[plugins]] tool_output_bytes`, when the config sets one. */
  declared_bytes?: number | null;
  granted_bytes?: number | null;
  decided_by?: string | null;
  /** Unix milliseconds. */
  decided_at?: number | null;
  reason?: string | null;
  pending?: { bytes: number; reason: string; requested_at: number } | null;
}

/** One recorded decision, with its audit trail, from `GET …/tool-output`. */
export interface PluginToolOutputRecord {
  plugin: string;
  effective_bytes: number;
  granted: boolean;
  global_bytes: number;
  ceiling_bytes: number;
  declared_bytes?: number | null;
  decision?: {
    decision: 'granted' | 'refused';
    bytes?: number | null;
    requested_bytes: number;
    reason: string;
    decided_by: string;
    channel: string;
    decided_at: number;
  } | null;
  pending?: { bytes: number; reason: string; requested_at: number } | null;
  history: {
    plugin: string;
    kind: string;
    bytes?: number | null;
    requested_bytes: number;
    reason: string;
    actor: string;
    at: number;
  }[];
}

/** What one plugin rescan changed. Empty arrays mean "nothing to do". */
export interface PluginRescanReport {
  added: string[];
  removed: string[];
  restarted: string[];
  unchanged: string[];
}

export interface PluginRescanResponse {
  report: PluginRescanReport;
  plugins: PluginDiagnostics[];
}
