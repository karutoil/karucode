import type { JSX } from 'preact';
import { useRef } from 'preact/hooks';
import { useFocusTrap } from '../lib/dom';
import * as actions from '../state/actions';
import type { InspectorTab } from '../state/store';
import * as store from '../state/store';
import { TabPanel, Tabs, type TabItem } from './Tabs';
import { WorkspaceTab } from './WorkspaceTab';
import { ChangesTab } from './ChangesTab';
import { CommandsTab } from './CommandsTab';
import { AgentsTab } from './AgentsTab';
import { CloseIcon, PanelIcon } from './icons';

export function Inspector(): JSX.Element {
  const active = store.inspectorTab.value;
  const isSheetOpen = store.mobileSheet.value === 'inspector';
  const isNarrow = store.narrowViewport.value;
  const inspectorRef = useRef<HTMLElement>(null);
  useFocusTrap(inspectorRef, isNarrow && isSheetOpen, actions.closeSheets);
  const items: TabItem<InspectorTab>[] = [
    { id: 'workspace', label: 'Workspace' },
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
          <span class="eyebrow">Inspector</span>
          <button
            type="button"
            class="icon-button"
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
      </div>
    </aside>
  );
}