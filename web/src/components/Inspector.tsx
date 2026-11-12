import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { getGitStatus } from '../api/endpoints';
import type { GitStatus } from '../api/types';
import { useFocusTrap } from '../lib/dom';
import { pluralise } from '../lib/format';
import * as actions from '../state/actions';
import type { InspectorTab } from '../state/store';
import * as store from '../state/store';
import * as workbench from '../state/workbench';
import { TabPanel, Tabs, type TabItem } from './Tabs';
import { WorkspaceTab } from './WorkspaceTab';
import { GitManager } from './git/GitManager';
import { changeCounts } from './git/gitModel';
import { SessionInfoTab, sessionInfoTooltip } from './SessionInfoTab';
import { AgentsTab } from './AgentsTab';
import { SkillsTab } from './SkillsTab';
import { PluginPanelView } from './PluginPanelView';
import { PluginSlot } from './PluginSlot';
import { uniqueHostedPanels } from './pluginPanels';
import {
  UI_HIDE_INSPECTOR_TAB_AGENTS,
  UI_HIDE_INSPECTOR_TAB_CHANGES,
  UI_HIDE_INSPECTOR_TAB_SESSION,
  UI_HIDE_INSPECTOR_TAB_WORKSPACE,
  UI_SLOT_INSPECTOR_HEADER,
  visibleTabs,
} from './uiContributions';
import { BranchIcon, CircleCheckIcon, CloseIcon, PanelIcon, pluginPanelIcon } from './icons';

/**
 * The hide target of a built-in inspector tab, or null for a plugin tab.
 */
function inspectorTabTarget(id: InspectorTab): string | null {
  switch (id) {
    case 'workspace':
      return UI_HIDE_INSPECTOR_TAB_WORKSPACE;
    case 'changes':
      return UI_HIDE_INSPECTOR_TAB_CHANGES;
    case 'session':
      return UI_HIDE_INSPECTOR_TAB_SESSION;
    case 'agents':
      return UI_HIDE_INSPECTOR_TAB_AGENTS;
    case 'skills':
      // Skills are text that reaches the model's prompt: a plugin may not hide
      // the only place they can be seen, turned off or removed. `null` is the
      // "not externally hideable" answer the plugin tabs also get.
      return null;
    default:
      // A plugin's own tab is not hideable from the outside: its plugin already
      // decides whether it declares one.
      return null;
  }
}

/**
 * Bottom git strip. It only renders when the server reports a real repository
 * and branch, so a non-git workspace never shows fabricated state.
 *
 * The counts come from the same `changeCounts` the manager uses, so the two
 * cannot disagree about what "staged" or "untracked" means, and the strip
 * re-probes when the window regains focus - the agent writes files while this
 * strip is showing the previous numbers.
 */
function BranchFooter(): JSX.Element | null {
  const projectId = store.currentProjectId.value;
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [probe, setProbe] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    setStatus(null);
    getGitStatus(projectId, { signal: controller.signal })
      .then((result) => {
        if (!controller.signal.aborted) setStatus(result);
      })
      .catch(() => {
        // No footer when git is unavailable; the Git tab owns the loud error.
      });
    return () => controller.abort();
  }, [projectId, probe]);

  useEffect(() => {
    if (typeof window === 'undefined') return;
    const onFocus = (): void => {
      if (document.visibilityState === 'visible') setProbe((count) => count + 1);
    };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, []);

  if (!status || !status.is_repo || !status.branch) return null;

  const counts = changeCounts(status.files);
  const parts = [
    counts.staged > 0 ? `${counts.staged} staged` : null,
    counts.changes > 0 ? `${counts.changes} modified` : null,
    counts.untracked > 0 ? `${counts.untracked} untracked` : null,
    counts.conflicts > 0 ? `${counts.conflicts} conflicted` : null,
  ].filter((part): part is string => part !== null);

  return (
    <div class="side-bottom-panel">
      <div class="branch-line">
        <BranchIcon size={13} />
        <span class="branch-label" title={status.branch}>
          {status.branch}
        </span>
      </div>
      <div class="branch-status">
        <span>
          <CircleCheckIcon size={11} />
          {counts.total === 0 ? 'Working tree clean' : pluralise(counts.total, 'changed file')}
        </span>
        {parts.length > 0 ? <span>{parts.join(' · ')}</span> : null}
      </div>
    </div>
  );
}

export function Inspector(): JSX.Element {
  const active = store.inspectorTab.value;
  const isSheetOpen = store.mobileSheet.value === 'inspector';
  const isNarrow = store.narrowViewport.value;
  const inspectorRef = useRef<HTMLElement>(null);
  const contextOpen = workbench.contextOpen.value;
  // A popped-out panel is an overlay that is closed as often as it is open, so
  // its only close control is an explicit one. On a phone the sheet always closes.
  const popout = !isNarrow && store.inspectorMode.value === 'popout';
  const contextSuppressesSheet = isNarrow && isSheetOpen && contextOpen;
  // Context is the top phone surface. The result sheet remains mounted, but must
  // not run a second focus trap or expose its controls behind the drawer.
  useFocusTrap(inspectorRef, isNarrow && isSheetOpen && !contextOpen, actions.closeSheets);
  const hostedPanels = uniqueHostedPanels(store.pluginPanelTabs.value);
  const pluginTabs = hostedPanels.panels;
  const ui = store.pluginUi.value;
  // Built-in tabs first, minus anything a plugin asked the app to hide. The last
  // surviving tab is never removed: an empty strip is worse than an ignored
  // request, and the plugin diagnostics say the request was made.
  const builtins = visibleTabs<TabItem<InspectorTab>>(
    [
      { id: 'workspace', label: 'Files' },
      { id: 'changes', label: 'Git', title: 'Source control — stage, commit, branches, history' },
      // The session's own numbers, not a second place to read commands: the
      // transcript already renders every command card inline, with its output.
      { id: 'session', label: 'Session usage', title: sessionInfoTooltip(store.sessionInfo.value) },
      { id: 'agents', label: 'Agents', count: store.runningAgents.value },
    ],
    ui,
    (id) => inspectorTabTarget(id),
  );
  const items: TabItem<InspectorTab>[] = [
    ...builtins.tabs,
    // Skills is a management surface, not a live feed: it is here next to Agents
    // because both answer "what can the model use?". It is appended outside
    // `visibleTabs` because it is not externally hideable (`inspectorTabTarget`
    // returns null for it): a plugin may not remove the one place skill text can
    // be seen or turned off. That also keeps the "no plugin may empty the strip"
    // refusal armed, since the decision is taken over the four hideable tabs.
    { id: 'skills', label: 'Skills' },
    // One tab per panel a plugin declared. The plugin supplies a data payload,
    // never markup: see `pluginPanels.ts` for why that matters.
    ...pluginTabs.map((panel) => ({
      id: panel.hosted_id,
      label: panel.title,
      icon: pluginPanelIcon(panel.icon, { size: 13 }),
    })),
  ];

  // A panel that disappears (its plugin was disabled, or the tab list arrived
  // after a boot) must not leave the inspector pointing at nothing. The same goes
  // for a tab a plugin just hid while it was the one on screen.
  useEffect(() => {
    if (items.some((item) => item.id === active)) return;
    store.inspectorTab.value = items[0]?.id ?? 'workspace';
  }, [active, items.map((item) => item.id).join('|')]);

  return (
    <aside
      ref={inspectorRef}
      id="workbench-results"
      class={`inspector${isSheetOpen ? ' is-open' : ''}`}
      aria-label="Workspace inspector"
      aria-hidden={contextSuppressesSheet ? 'true' : undefined}
      inert={contextSuppressesSheet ? true : undefined}
      role={isSheetOpen || popout ? 'dialog' : undefined}
      aria-modal={isSheetOpen && !contextOpen ? 'true' : undefined}
      data-context-open={contextOpen ? 'true' : 'false'}
      data-results-popout={popout ? 'true' : undefined}
    >
      <div class="inspector-inner">
        <div class="inspector-top">
          <div class="inspector-head">
            <span class="inspector-label">Results</span>
          </div>
          <div class="inspector-head-actions">
            {/* Controls plugins contributed to the results header. Context is not
              * one of them: it lives in the session header, so it stays reachable
              * whether this panel is docked, popped out or closed. */}
            <PluginSlot slot={UI_SLOT_INSPECTOR_HEADER} />
            <button
              type="button"
              class="icon-button inspector-quick"
              onClick={actions.toggleInspector}
              aria-label={isNarrow ? 'Close inspector' : popout ? 'Close results panel' : 'Collapse inspector'}
              title={
                isNarrow
                  ? 'Close the results sheet'
                  : popout
                    ? 'Close the results panel'
                    : 'Show or hide the results dock'
              }
              data-autofocus={isNarrow && isSheetOpen && !contextOpen ? true : undefined}
            >
              {isNarrow || popout ? <CloseIcon /> : <PanelIcon />}
            </button>
          </div>
        </div>

        <Tabs
          items={items}
          active={active}
          onChange={actions.selectInspectorTab}
          label="Inspector sections"
          idPrefix="inspector"
        />
        {hostedPanels.duplicates.length > 0 ? (
          <p class="inline-warning" role="alert">
            {hostedPanels.duplicates.length} legacy plugin panel{hostedPanels.duplicates.length === 1 ? '' : 's'}
            {' '}used a duplicate hosted id and {hostedPanels.duplicates.length === 1 ? 'was' : 'were'} omitted
            to prevent two plugins aliasing one inspector tab. Reload the plugins to get fresh diagnostics.
          </p>
        ) : null}

        <TabPanel idPrefix="inspector" id="workspace" active={active === 'workspace'}>
          <WorkspaceTab />
        </TabPanel>
        <TabPanel idPrefix="inspector" id="changes" active={active === 'changes'}>
          <div class="result-git">
            <GitManager variant="page" />
          </div>
        </TabPanel>
        <TabPanel idPrefix="inspector" id="session" active={active === 'session'}>
          <SessionInfoTab />
        </TabPanel>
        <TabPanel idPrefix="inspector" id="agents" active={active === 'agents'}>
          <AgentsTab />
        </TabPanel>
        <TabPanel idPrefix="inspector" id="skills" active={active === 'skills'}>
          <SkillsTab />
        </TabPanel>
        {pluginTabs.map((panel) => (
          <TabPanel
            key={panel.hosted_id}
            idPrefix="inspector"
            id={panel.hosted_id}
            active={active === panel.hosted_id}
          >
            <PluginPanelView panel={panel} />
          </TabPanel>
        ))}

        <BranchFooter />
      </div>
    </aside>
  );
}