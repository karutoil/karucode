import type { JSX } from 'preact';
import type { Project } from '../../api/types';
import { copyText } from '../../lib/clipboard';
import { Menu, MenuItem, type MenuController } from '../../lib/menu';
import * as actions from '../../state/actions';
import * as store from '../../state/store';
import { KebabIcon } from '../../components/icons';
import { IconButton } from '../../ui';

export interface ProjectOptionsMenuProps {
  activeProject: Project | null;
  menu: MenuController;
  onRename: () => void;
  onDelete: () => void;
}

export function ProjectOptionsMenu({
  activeProject,
  menu,
  onRename,
  onDelete,
}: ProjectOptionsMenuProps): JSX.Element {
  const copyPath = async (): Promise<void> => {
    const path = activeProject?.path ?? '';
    if (path === '') return;
    const copied = await copyText(path);
    store.pushToast(
      copied ? 'Project path copied.' : 'Could not copy the path.',
      copied ? 'success' : 'error',
    );
  };

  return (
    <div class="project-switcher-menu-wrap">
      <IconButton
        size="md"
        class="side-mini-btn w-[28px] h-[28px]"
        label="Project options"
        title="Project options"
        disabled={!activeProject}
        buttonRef={menu.triggerProps.ref}
        aria-haspopup="menu"
        expanded={menu.triggerProps['aria-expanded']}
        icon={<KebabIcon size={15} />}
        testId="project-options-trigger"
        onClick={menu.triggerProps.onClick}
      />
      {activeProject ? (
        <Menu menu={menu} label={`Project options for ${activeProject.name}`} class="rail-menu project-switcher-menu">
          <div class="rail-menu-meta">
            <span class="rail-menu-name">{activeProject.name}</span>
            <span class="rail-menu-path">{activeProject.path}</span>
          </div>
          <MenuItem
            menu={menu}
            action={{
              label: 'Copy project path',
              disabled: !activeProject.path,
              onSelect: () => void copyPath(),
            }}
          />
          <MenuItem
            menu={menu}
            action={{ label: 'Refresh sessions', onSelect: () => void actions.refreshSessions() }}
          />
          <MenuItem
            menu={menu}
            action={{ label: 'Project settings…', onSelect: actions.openSettings }}
          />
          <MenuItem menu={menu} action={{ label: 'Rename…', onSelect: onRename }} />
          <MenuItem
            menu={menu}
            action={{ label: 'Delete project…', danger: true, onSelect: onDelete }}
          />
        </Menu>
      ) : null}
    </div>
  );
}
