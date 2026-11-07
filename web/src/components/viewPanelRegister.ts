/**
 * Registers the panel *views* vocabulary, for every plugin.
 *
 * Imported for its side effect from `main.tsx`, before the first render. Unlike the
 * per-plugin registrations beside this file, this one is not tied to a plugin id: it
 * claims any payload whose shape follows the documented `view` vocabulary
 * (`{"view": "stat", …}`, `{"view": "bars", …}`, a live `{"view": "clock", …}`), so
 * a plugin can draw a real view without the app knowing it exists and without
 * shipping a line of UI.
 *
 * A payload with no `view` — or one this build does not know — keeps the generic
 * JSON view, which is the property that lets an older app and a newer plugin read
 * each other.
 */
import { registerPanelShapeRenderer } from './pluginPanels';
import { pluginViewRenderer } from './PluginViews';

registerPanelShapeRenderer(pluginViewRenderer);
