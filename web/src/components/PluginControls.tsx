import type { JSX } from 'preact';
import * as store from '../state/store';
import {
  hasUiOverrides,
  pluginUiPreferences,
  resetPluginUiPreferences,
  setContributionEnabled,
  setHideOverridden,
} from '../state/pluginUiPreferences';
import { Toggle } from './Toggle';
import type { UiContribution } from './uiContributions';

/**
 * The person's side of the plugin-UI bargain.
 *
 * A plugin can add controls to the interface and can ask the app to hide one of its
 * own. Without this panel the only way to undo either is to disable the whole
 * plugin — which also takes away its tools and its hooks. So this lists what is
 * currently contributed, per plugin, with one switch per control, and every
 * built-in a plugin hid, with a switch that draws it again.
 *
 * It is deliberately a *browser* preference (localStorage, like the theme): it needs
 * no approval, no round trip and no config reload, and the note says so, because
 * "why is my other browser different" is the obvious next question.
 *
 * The list comes from `store.pluginUiDeclared` — what the plugins actually asked
 * for — not from the filtered view, otherwise switching a control off would remove
 * the switch that turns it back on.
 */

/** How a built-in control is named to a person, not to a plugin author. */
const TARGET_LABEL: Record<string, string> = {
  'composer.attach': 'the attach (paperclip) button',
  'inspector.tab.workspace': 'the Files tab',
  'inspector.tab.changes': 'the Git tab',
  'inspector.tab.session': 'the Session usage tab',
  'inspector.tab.agents': 'the Agents tab',
  'inspector.context': 'the Context button in the session header',
};

/** Where a contributed control appears, in words. */
function whereLabel(contribution: UiContribution): string {
  if (contribution.kind === 'command') return 'command palette';
  if (contribution.kind === 'keybinding') {
    return contribution.keys ? `keyboard · ${contribution.keys}` : 'keyboard shortcut';
  }
  switch (contribution.slot) {
    case 'composer.toolbar':
      return 'composer';
    case 'inspector.header':
      return 'results header';
    case 'session.header':
      return 'session header';
    default:
      return contribution.slot ?? 'interface';
  }
}

function targetLabel(target: string): string {
  return TARGET_LABEL[target] ?? target;
}

function ContributionRow({ contribution }: { contribution: UiContribution }): JSX.Element {
  const prefs = pluginUiPreferences.value;
  const enabled = !prefs.disabled.includes(contribution.hosted_id);
  return (
    <li class="plugin-control-row" data-plugin={contribution.plugin}>
      <Toggle
        checked={enabled}
        onChange={(next) => setContributionEnabled(contribution.hosted_id, next)}
        label={contribution.label}
        description={`${contribution.plugin} · ${contribution.kind} · ${whereLabel(contribution)}`}
        id={`plugin-control-${contribution.plugin}-${contribution.id}`}
      />
    </li>
  );
}

function HiddenRow({ target, plugins }: { target: string; plugins: string[] }): JSX.Element {
  const prefs = pluginUiPreferences.value;
  // The switch reads "draw it again", so checked means the person overrode the
  // plugin's request. That is the action they came here to take.
  const shownAgain = prefs.shown.includes(target);
  return (
    <li class="plugin-control-row" data-hide-target={target}>
      <Toggle
        checked={shownAgain}
        onChange={(next) => setHideOverridden(target, next)}
        label={`Show ${targetLabel(target)} again`}
        description={`Hidden by ${plugins.join(', ')}`}
        id={`plugin-hide-${target.replace(/[^a-z0-9]+/gi, '-')}`}
      />
    </li>
  );
}

export function PluginControls(): JSX.Element {
  const declared = store.pluginUiDeclared.value;
  const prefs = pluginUiPreferences.value;
  const byPlugin = new Map<string, UiContribution[]>();
  for (const contribution of declared.contributions) {
    const list = byPlugin.get(contribution.plugin) ?? [];
    list.push(contribution);
    byPlugin.set(contribution.plugin, list);
  }
  const hidden = Object.entries(declared.hidden);
  const empty = byPlugin.size === 0 && hidden.length === 0;
  const overridden = hasUiOverrides(prefs);

  return (
    <div class="plugin-controls" data-testid="plugin-controls">
      <div class="settings-section-head">
        <h4 id="settings-plugin-controls">Controls in the interface</h4>
        {overridden ? (
          <button
            type="button"
            class="button button-small"
            onClick={() => resetPluginUiPreferences()}
            title="Honour every plugin's declaration again"
          >
            Reset to what the plugins ask for
          </button>
        ) : null}
      </div>
      <p class="settings-section-note" id="settings-plugin-controls-note">
        {empty
          ? 'No plugin adds controls to the interface right now.'
          : 'A plugin can add buttons and palette commands, and can ask the app to hide one of its own controls. Switch a contributed control off, or put a hidden one back. These choices are stored in this browser — they do not follow you to another device, and they need no approval because nothing here calls the plugin.'}
      </p>
      {byPlugin.size > 0 ? (
        <ul class="plugin-controls-list" aria-labelledby="settings-plugin-controls">
          {[...byPlugin.entries()].map(([plugin, contributions]) => (
            <li class="plugin-controls-group" key={plugin}>
              <span class="eyebrow">{plugin}</span>
              <ul class="plugin-controls-items">
                {contributions.map((contribution) => (
                  <ContributionRow key={contribution.hosted_id} contribution={contribution} />
                ))}
              </ul>
            </li>
          ))}
        </ul>
      ) : null}
      {hidden.length > 0 ? (
        <ul class="plugin-controls-list" aria-label="Controls hidden by plugins">
          {hidden.map(([target, plugins]) => (
            <HiddenRow key={target} target={target} plugins={plugins} />
          ))}
        </ul>
      ) : null}
    </div>
  );
}
