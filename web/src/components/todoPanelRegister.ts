/**
 * Registers the first-party renderer for the `kc-todo` plugin's inspector panel.
 *
 * Imported for its side effect from `main.tsx`, before the first render. A
 * plugin ships the panel's *data* and never its UI, so the app owns the drawing:
 * `plugin__kc-todo__todo` is the hosted id the inspector gives the panel, and
 * registering the hosted id (rather than the bare `todo`) keeps another
 * plugin's panel of the same name on the generic JSON view.
 *
 * Without this file the tab still appears — the shell falls back to a read-only
 * JSON view of the payload. With it, the payload is drawn as a task tree.
 */
import { registerPluginPanelRenderer } from './pluginPanels';
import { TODO_PANEL_ID, todoPanelRenderer } from './TodoPanel';

registerPluginPanelRenderer(TODO_PANEL_ID, todoPanelRenderer);
