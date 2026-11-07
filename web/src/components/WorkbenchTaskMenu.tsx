import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { Menu, MenuItem, useMenu } from '../lib/menu';
import * as actions from '../state/actions';
import * as store from '../state/store';
import * as workbench from '../state/workbench';
import { controlPlaneOpen } from '../state/controlPlane';
import { modKey } from '../lib/platform';
import {
  CopyIcon,
  KebabIcon,
  SearchIcon,
  SettingsIcon,
} from './icons';

export interface WorkbenchTaskMenuProps {
  canCopy: boolean;
  onCopy: () => void;
  /** The control plane's own conversation has no workbench context to open. */
  controlPlane?: boolean;
}

/**
 * One accessible home for secondary task and workspace actions.
 *
 * The menu's behaviour - open focuses the first enabled item, arrows/Home/End walk
 * enabled items, Escape and Tab return focus to the trigger, a click outside
 * closes - is `lib/menu.tsx`'s, shared with every other menu in the product. This
 * component owns only what is on the menu.
 */
export function WorkbenchTaskMenu({
  canCopy,
  onCopy,
  controlPlane = false,
}: WorkbenchTaskMenuProps): JSX.Element {
  const menu = useMenu();
  const session = store.currentSessionId.value;

  // A new conversation invalidates whatever the menu was about.
  useEffect(() => menu.close(), [session]);

  return (
    <div class="workbench-task-menu">
      <button
        type="button"
        class="head-btn head-icon"
        aria-label="Task actions"
        title="Task actions"
        {...menu.triggerProps}
      >
        <KebabIcon size={15} />
      </button>
      <Menu menu={menu} label="Task and workspace actions" class="workbench-task-menu-popover">
        <MenuItem
          menu={menu}
          action={{ label: 'Copy conversation', icon: <CopyIcon size={13} />, disabled: !canCopy, onSelect: onCopy }}
        />
        {!controlPlane ? (
          <MenuItem
            menu={menu}
            action={{
              label: 'Session context',
              icon: <SettingsIcon size={13} />,
              onSelect: () => workbench.openContext('now'),
            }}
          />
        ) : null}
        <MenuItem
          menu={menu}
          action={{
            label: 'Commands',
            keyshortcuts: 'Control+K Meta+K',
            onSelect: actions.togglePalette,
          }}
        >
          <SearchIcon size={13} /> Commands <kbd>{modKey()} K</kbd>
        </MenuItem>
        <span class="workbench-task-menu-rule" role="separator" />
        <MenuItem
          menu={menu}
          action={{
            label: 'Theme studio',
            keyshortcuts: 'Control+Shift+T Meta+Shift+T',
            onSelect: actions.openThemeStudio,
          }}
        />
        <MenuItem
          menu={menu}
          action={{ label: 'Settings', icon: <SettingsIcon size={13} />, onSelect: actions.openSettings }}
        />
        {!controlPlane ? (
          <MenuItem
            menu={menu}
            action={{
              label: 'All tasks',
              icon: <SearchIcon size={13} />,
              onSelect: () => {
                controlPlaneOpen.value = true;
              },
            }}
          />
        ) : null}
      </Menu>
    </div>
  );
}
