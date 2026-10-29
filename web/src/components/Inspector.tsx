import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { getGitStatus } from '../api/endpoints';
import type { GitStatus } from '../api/types';
import { useFocusTrap } from '../lib/dom';
import { pluralise } from '../lib/format';
import * as actions from '../state/actions';
import type { InspectorTab } from '../state/store';
import * as store from '../state/store';
import { TabPanel, Tabs, type TabItem } from './Tabs';
import { WorkspaceTab } from './WorkspaceTab';
import { ChangesTab } from './ChangesTab';
import { CommandsTab } from './CommandsTab';
import { AgentsTab } from './AgentsTab';
import { BranchIcon, CircleCheckIcon, CloseIcon, PanelIcon } from './icons';

/**
 * Bottom git strip. It only renders when the server reports a real repository
 * and branch, so a non-git workspace never shows fabricated state.
 */
function BranchFooter(): JSX.Element | null {
  const projectId = store.currentProjectId.value;
  const [status, setStatus] = useState<GitStatus | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    setStatus(null);
    getGitStatus(projectId, { signal: controller.signal })
      .then((result) => {
        if (!controller.signal.aborted) setStatus(result);
      })
      .catch(() => {
        // No footer when git is unavailable; ChangesTab owns the loud error.
      });
    return () => controller.abort();
  }, [projectId]);

  if (!status || !status.is_repo || !status.branch) return null;

  const changed = status.files.length;
  const staged = status.files.filter((file) => file.staged).length;
  const unstaged = changed - staged;

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
          {changed === 0 ? 'Working tree clean' : pluralise(changed, 'modified file')}
        </span>
        {changed > 0 ? (
          <span>
            {staged} staged · {unstaged} unstaged
          </span>
        ) : null}
      </div>
    </div>
  );
}

export function Inspector(): JSX.Element {
  const active = store.inspectorTab.value;
  const isSheetOpen = store.mobileSheet.value === 'inspector';
  const isNarrow = store.narrowViewport.value;
  const inspectorRef = useRef<HTMLElement>(null);
  useFocusTrap(inspectorRef, isNarrow && isSheetOpen, actions.closeSheets);
  const items: TabItem<InspectorTab>[] = [
    { id: 'workspace', label: 'Files' },
    { id: 'changes', label: 'Changes' },
    {
      id: 'commands',
      label: 'Commands',
      count: store.chat.value.items.filter((item) => item.kind === 'command').length,
    },
    { id: 'agents', label: 'Agents', count: store.chat.value.agents.length },
  ];

  return (
    <aside
      ref={inspectorRef}
      class={`inspector${isSheetOpen ? ' is-open' : ''}`}
      aria-label="Workspace inspector"
      role={isSheetOpen ? 'dialog' : undefined}
      aria-modal={isSheetOpen ? 'true' : undefined}
    >
      <div class="inspector-inner">
        <div class="inspector-top">
          <div class="inspector-head">
            <span class="inspector-label">Inspector</span>
            <span class="inspector-subtitle">Project context, always in reach</span>
          </div>
          <button
            type="button"
            class="icon-button inspector-quick"
            onClick={actions.toggleInspector}
            aria-label={isNarrow ? 'Close inspector' : 'Collapse inspector'}
            title="Toggle inspector — Cmd/Ctrl+J"
            data-autofocus={isNarrow && isSheetOpen ? true : undefined}
          >
            {store.narrowViewport.value ? <CloseIcon /> : <PanelIcon />}
          </button>
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
          <ChangesTab />
        </TabPanel>
        <TabPanel idPrefix="inspector" id="commands" active={active === 'commands'}>
          <CommandsTab />
        </TabPanel>
        <TabPanel idPrefix="inspector" id="agents" active={active === 'agents'}>
          <AgentsTab />
        </TabPanel>

        <BranchFooter />
      </div>
    </aside>
  );
}