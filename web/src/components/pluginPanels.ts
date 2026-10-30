/**
 * The registry that lets a plugin's panel data be rendered by a first-party
 * component.
 *
 * A plugin cannot ship UI: the frontend is a compile-time snapshot embedded in
 * the binary, so nothing external can add a tab at runtime. What a plugin *can*
 * do is declare a panel in its manifest and answer `panels/read` with arbitrary
 * JSON, which the host forwards untouched. This module is the other half of
 * that contract — a place for the app to say "payloads shaped like this are
 * drawn like that".
 *
 * The payload is **untrusted**. A renderer receives it as `unknown`, must
 * validate it before use, and must never pass it to `dangerouslySetInnerHTML`,
 * build a URL from it, or hand it to `eval`. Nothing here executes plugin code
 * in the app's origin; that property is what makes the CSP and
 * `frame-ancestors 'none'` meaningful.
 */

import type { JSX } from 'preact';

/** One plugin panel as `GET /api/config` describes it. */
export interface PluginPanelSummary {
  /** Bare id from the plugin manifest (`todo`). */
  id: string;
  /** `plugin__<plugin>__<id>`: the inspector tab id and the request path. */
  hosted_id: string;
  title: string;
  icon?: string | null;
  /** `tool` or `static`; informational, both are read the same way. */
  source: string;
  /** Suggested poll interval in ms. 0 means "only on explicit refresh". */
  refresh_ms: number;
}

/** One plugin's panels, as grouped by `GET /api/config`. */
export interface PluginPanelGroup {
  plugin: string;
  version: string;
  panels: PluginPanelSummary[];
}

/** What a renderer is handed when a panel payload has been fetched. */
export interface PluginPanelRenderProps {
  /** The declaration, for a title, a badge or a key. */
  panel: PluginPanelSummary;
  /** The plugin that owns the panel. */
  plugin: string;
  /** The plugin's version, for an "out of date" hint. */
  version: string;
  /**
   * The payload from `GET /api/plugins/:name/panels/:panel`.
   *
   * `unknown` on purpose: it is whatever JSON the plugin returned. Validate it
   * (`matches` is where that normally happens) before reading any field.
   */
  data: unknown;
  /** Ask the shell to fetch this panel again (a "retry" or "refresh" button). */
  refresh: () => void;
}

/** A component that can draw one plugin's panel payload. */
export interface PluginPanelRenderer {
  /**
   * Decide whether this renderer claims `data`. It is called once per payload,
   * before rendering, so a renderer that cannot read the shape should return
   * `false` and let the generic JSON view take over rather than throwing.
   */
  matches(data: unknown): boolean;
  render(props: PluginPanelRenderProps): JSX.Element;
}

/** The registry: first registration for an id wins, so core can pre-empt one. */
const renderers = new Map<string, PluginPanelRenderer>();

/** Payloads are capped so a hostile or broken plugin cannot freeze the tab. */
export const MAX_PANEL_PAYLOAD_CHARS = 256 * 1024;

/**
 * Register a renderer for a panel id (`plugin__<plugin>__<id>`, or the bare
 * `<id>` when a plugin wants every panel of that name drawn the same way).
 *
 * Registering the same id twice replaces the earlier renderer: a later
 * registration is almost always a deliberate override (theme, feature flag,
 * test), and silently keeping the first would be a confusing bug.
 */
export function registerPluginPanelRenderer(id: string, renderer: PluginPanelRenderer): void {
  renderers.set(id, renderer);
}

/** Remove a renderer. Returns whether one was registered. */
export function unregisterPluginPanelRenderer(id: string): boolean {
  return renderers.delete(id);
}

/**
 * The renderer for `panel`, or `undefined` when the app has none.
 *
 * The hosted id wins over the bare id so a plugin can style one panel without
 * affecting another plugin's panel of the same name.
 */
export function findPluginPanelRenderer(
  panel: PluginPanelSummary,
): PluginPanelRenderer | undefined {
  return renderers.get(panel.hosted_id) ?? renderers.get(panel.id);
}

/** True when a payload is small enough to serialise and draw. */
export function panelPayloadFits(data: unknown): boolean {
  try {
    const text = JSON.stringify(data);
    return text !== undefined && text.length <= MAX_PANEL_PAYLOAD_CHARS;
  } catch {
    // A cyclic or otherwise unserialisable payload cannot be drawn.
    return false;
  }
}

/** Reset the registry. Tests only: the app registers at import time. */
export function resetPluginPanelRenderers(): void {
  renderers.clear();
}

/**
 * The plugin and bare panel id behind a hosted id.
 *
 * `plugin__kc-todo__todo` → `{ plugin: 'kc-todo', panel: 'todo' }`. A plugin
 * name may itself contain `__`, so the *last* separator wins — the same rule the
 * host uses when it namespaces a tool.
 */
export function splitHostedPanelId(hostedId: string): { plugin: string; panel: string } | null {
  const prefix = 'plugin__';
  if (!hostedId.startsWith(prefix)) return null;
  const rest = hostedId.slice(prefix.length);
  const at = rest.lastIndexOf('__');
  if (at <= 0 || at + 2 >= rest.length) return null;
  return { plugin: rest.slice(0, at), panel: rest.slice(at + 2) };
}

/** How a panel's `refresh_ms` is interpreted: 0 means "manual only". */
export function panelPollInterval(panel: PluginPanelSummary): number {
  // The server already clamps to 1000..=300000, but a client must not trust it:
  // a bug there would turn into a request loop here.
  if (!panel.refresh_ms || panel.refresh_ms <= 0) return 0;
  return Math.min(Math.max(panel.refresh_ms, 1_000), 300_000);
}
