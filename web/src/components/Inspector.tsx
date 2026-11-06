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
import { PluginPanelView } from './PluginPanelView';
import { BranchIcon, CircleCheckIcon, CloseIcon, LayersIcon, PanelIcon, pluginPanelIcon } from './icons';

/**
 * Refresh a plugin panel when the user asks for it. Kept next to the tabs so the
 * tab strip has a single place that knows a plugin tab is not a built-in one.
 */
function isPluginTab(id: InspectorTab): boolean {
  return id.startsWith('plugin__');
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
  const contextSuppressesSheet = isNarrow && isSheetOpen && contextOpen;
  // Context is the top phone surface. The result sheet remains mounted, but must
  // not run a second focus trap or expose its controls behind the drawer.
  useFocusTrap(inspectorRef, isNarrow && isSheetOpen && !contextOpen, actions.closeSheets);
  const pluginTabs = store.pluginPanelTabs.value;
  const items: TabItem<InspectorTab>[] = [
    { id: 'workspace', label: 'Files' },
    { id: 'changes', label: 'Git', title: 'Source control — stage, commit, branches, history' },
    // The session's own numbers, not a second place to read commands: the
    // transcript already renders every command card inline, with its output.
    { id: 'session', label: 'Session usage', title: sessionInfoTooltip(store.sessionInfo.value) },
    { id: 'agents', label: 'Agents', count: store.runningAgents.value },
    // One tab per panel a plugin declared. The plugin supplies a data payload,
    // never markup: see `pluginPanels.ts` for why that matters.
    ...pluginTabs.map((panel) => ({
      id: panel.hosted_id,
      label: panel.title,
      icon: pluginPanelIcon(panel.icon, { size: 13 }),
    })),
  ];

  // A panel that disappears (its plugin was disabled, or the tab list arrived
  // after a boot) must not leave the inspector pointing at nothing.
  useEffect(() => {
    if (!isPluginTab(active)) return;
    if (!pluginTabs.some((panel) => panel.hosted_id === active)) {
      store.inspectorTab.value = 'workspace';
    }
  }, [active, pluginTabs]);

  return (
    <aside
      ref={inspectorRef}
      id="workbench-results"
      class={`inspector${isSheetOpen ? ' is-open' : ''}`}
      aria-label="Workspace inspector"
      aria-hidden={contextSuppressesSheet ? 'true' : undefined}
      inert={contextSuppressesSheet ? true : undefined}
      role={isSheetOpen ? 'dialog' : undefined}
      aria-modal={isSheetOpen && !contextOpen ? 'true' : undefined}
      data-context-open={contextOpen ? 'true' : 'false'}
    >
      <div class="inspector-inner">
        <div class="inspector-top">
          <div class="inspector-head">
            <span class="inspector-label">Results</span>
          </div>
          <div class="inspector-head-actions">
            <button
              type="button"
              class="button button-small inspector-context-button"
              onClick={() => workbench.openContext('now')}
              aria-haspopup="dialog"
              aria-expanded={contextOpen}
              title="Current workspace, session, usage and access"
            >
              <LayersIcon size={13} />
              <span>Context</span>
            </button>
            <button
              type="button"
              class="icon-button inspector-quick"
              onClick={actions.toggleInspector}
              aria-label={isNarrow ? 'Close inspector' : 'Collapse inspector'}
              title={isNarrow ? 'Close the results sheet' : 'Show or hide the results dock'}
              data-autofocus={isNarrow && isSheetOpen && !contextOpen ? true : undefined}
            >
              {isNarrow ? <CloseIcon /> : <PanelIcon />}
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