import { useEffect, useId, useRef, useState } from 'preact/hooks';
import type { JSX } from 'preact';
import { useMenu } from '../../lib/menu';
import * as actions from '../../state/actions';
import * as store from '../../state/store';
import { controlPlaneOpen } from '../../state/controlPlane';
import {
  AddProjectDialog,
  DeleteProjectDialog,
  ProjectGlyph,
  RenameProjectDialog,
  unreadLabel,
} from '../../components/ProjectRail';
import { ChevronDownIcon, CloseIcon, PlusIcon } from '../../components/icons';
import { IconButton, Text } from '../../ui';
import { PROJECTS_HOVER_CLOSE_MS, PROJECTS_HOVER_OPEN_MS } from './helpers';
import { ProjectOptionsMenu } from './ProjectOptionsMenu';

/**
 * One project switcher for both navigation layouts.
 *
 * The ordinary wide Workbench uses a compact active-project disclosure in this
 * column; the collapsed desktop mode still uses ProjectRail's real tiles and
 * RailPeek anchors. A phone keeps the existing full horizontal project list in
 * the sessions sheet. Both modes share the store, dialogs and actual commands.
 */
export interface ProjectSwitcherProps {
  /** Wide view: a compact disclosure in the one navigation column. */
  desktop?: boolean;
}

export function ProjectSwitcher({ desktop = false }: ProjectSwitcherProps): JSX.Element {
  const projects = store.projects.value;
  const activeId = store.currentProjectId.value;
  const activeProject = store.currentProject.value;
  const unreadByProject = store.unreadByProject.value;
  const [dialog, setDialog] = useState<'add' | 'rename' | 'delete' | null>(null);
  const menu = useMenu();
  const [projectsOpen, setProjectsOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const disclosureRef = useRef<HTMLButtonElement>(null);
  const disclosureId = useId();
  /** Whether resting on the trigger opens the list, or only a press does. */
  const hoverOpens = store.projectMenuOpen.value === 'hover';
  /** Hover timers for the desktop dropdown, plus whether the pointer is what
   *  opened it — a hover must never move focus. */
  const hoverOpenTimer = useRef<number | null>(null);
  const hoverCloseTimer = useRef<number | null>(null);
  const openedByHover = useRef(false);

  const clearHoverTimers = (): void => {
    if (hoverOpenTimer.current !== null) {
      window.clearTimeout(hoverOpenTimer.current);
      hoverOpenTimer.current = null;
    }
    if (hoverCloseTimer.current !== null) {
      window.clearTimeout(hoverCloseTimer.current);
      hoverCloseTimer.current = null;
    }
  };

  const closeProjects = (restoreFocus = true): void => {
    clearHoverTimers();
    openedByHover.current = false;
    setProjectsOpen(false);
    if (restoreFocus) {
      window.setTimeout(() => {
        if (disclosureRef.current?.isConnected) disclosureRef.current.focus();
      }, 0);
    }
  };

  /** The click and keyboard path: Enter/Space on the trigger lands here too. */
  const toggleProjects = (): void => {
    if (projectsOpen) {
      closeProjects(false);
      return;
    }
    clearHoverTimers();
    openedByHover.current = false;
    setProjectsOpen(true);
  };

  /** Resting on the trigger opens the dropdown (the way a menu bar does).
   *
   *  Only in `hover` mode: the setting exists because this is the one control in
   *  the rail that opens under a pointer which was only crossing it on the way to
   *  the composer. In `click` mode the trigger is an ordinary disclosure and the
   *  pointer may pass over it freely. */
  const hoverOpenProjects = (): void => {
    if (!desktop || !hoverOpens) return;
    clearHoverTimers();
    if (projectsOpen) return;
    hoverOpenTimer.current = window.setTimeout(() => {
      hoverOpenTimer.current = null;
      openedByHover.current = true;
      setProjectsOpen(true);
    }, PROJECTS_HOVER_OPEN_MS);
  };

  /** The pointer is back inside (the panel): cancel a pending close. */
  const holdProjects = (): void => {
    if (hoverCloseTimer.current !== null) {
      window.clearTimeout(hoverCloseTimer.current);
      hoverCloseTimer.current = null;
    }
  };

  /** Leaving the trigger or the panel closes it, after the grace period.
   *
   *  A `click` menu stays open until it is dismissed: closing it because the
   *  pointer moved would be the hover behaviour the setting just turned off. */
  const hoverCloseProjects = (): void => {
    if (!desktop || !hoverOpens) return;
    if (hoverOpenTimer.current !== null) {
      // Left before it opened: never open at all.
      window.clearTimeout(hoverOpenTimer.current);
      hoverOpenTimer.current = null;
    }
    if (!projectsOpen) return;
    holdProjects();
    hoverCloseTimer.current = window.setTimeout(() => {
      hoverCloseTimer.current = null;
      openedByHover.current = false;
      setProjectsOpen(false);
    }, PROJECTS_HOVER_CLOSE_MS);
  };

  useEffect(() => clearHoverTimers, []);

  useEffect(() => {
    if (!desktop || !projectsOpen) return;
    const onPointerDown = (event: MouseEvent): void => {
      if (!rootRef.current?.contains(event.target as Node)) closeProjects(false);
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      // The nested options menu owns its Escape first. Only once that is closed
      // does the next Escape dismiss the projects disclosure.
      if (event.key === 'Escape' && !menu.open) {
        event.preventDefault();
        event.stopPropagation();
        closeProjects();
      }
    };
    document.addEventListener('mousedown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [desktop, projectsOpen, menu.open]);

  useEffect(() => {
    if (!desktop || !projectsOpen) return;
    // A hover opens the list for the eyes only. Moving focus under a resting
    // pointer would pull the caret out of whatever the reader was typing in.
    if (openedByHover.current) {
      openedByHover.current = false;
      return;
    }
    const active = rootRef.current?.querySelector<HTMLButtonElement>(
      '.project-switcher-item[aria-current="true"]',
    );
    const first = rootRef.current?.querySelector<HTMLButtonElement>('.project-switcher-item');
    (active ?? first)?.focus();
  }, [desktop, projectsOpen]);

  // A project that disappears (deleted elsewhere) must not leave the menu open
  // pointing at nothing.
  useEffect(() => {
    if (!activeProject) menu.close();
  }, [activeProject]);

  const activeUnread = activeProject ? unreadByProject[activeProject.id] ?? 0 : 0;

  /** The project rows, shared by the desktop dropdown (which floats over the rail)
   *  and the phone sheet (where the list is part of the sheet). */
  const projectList = (
    <>
      <ul class="project-switcher-list">
        {projects.map((item) => {
          const active = item.id === activeId;
          const unread = unreadByProject[item.id] ?? 0;
          const unreadNote = unread > 0 ? `, ${unreadLabel(unread)}` : '';
          return (
            <li key={item.id}>
              <button
                type="button"
                class={`project-switcher-item${active ? ' is-active' : ''}`}
                aria-label={`Switch to ${item.name} (${item.path})${unreadNote}`}
                aria-current={active ? 'true' : undefined}
                title={`${item.path}${
                  item.session_count > 0
                    ? `\n${item.session_count} ${item.session_count === 1 ? 'session' : 'sessions'}`
                    : ''
                }${unread > 0 ? `\n${unreadLabel(unread)}` : ''}`}
                onClick={() => {
                  controlPlaneOpen.value = false;
                  if (desktop) closeProjects();
                  if (!active) void actions.switchProject(item.id);
                }}
              >
                <ProjectGlyph project={item} />
                <span class="project-switcher-name">{item.name}</span>
                {unread > 0 ? (
                  <span class="workbench-project-unread" aria-hidden="true">
                    {unread > 99 ? '99+' : unread}
                  </span>
                ) : null}
              </button>
            </li>
          );
        })}
      </ul>
      {desktop && projects.length === 0 ? (
        <Text as="p" size="tiny" tone="muted" class="workbench-projects-empty">
          No projects yet. Use + to add one.
        </Text>
      ) : null}
    </>
  );

  return (
    <div
      ref={rootRef}
      class={`project-switcher${desktop ? ' project-switcher-desktop' : ''}`}
    >
      <div class={`project-switcher-head${desktop ? ' project-heading' : ''}`}>
        {desktop ? (
          <button
            ref={disclosureRef}
            type="button"
            class="workbench-project-disclosure project-title"
            aria-label={`Projects: ${activeProject?.name ?? 'none selected'}${
              activeUnread > 0 ? `, ${unreadLabel(activeUnread)}` : ''
            }`}
            aria-expanded={projectsOpen}
            aria-controls={disclosureId}
            title={activeProject?.path ?? 'Choose a project'}
            onClick={toggleProjects}
            onMouseEnter={hoverOpenProjects}
            onMouseLeave={hoverCloseProjects}
          >
            {activeProject ? <ProjectGlyph project={activeProject} /> : null}
            <span class="workbench-project-copy">
              <span class="project-title-text">{activeProject?.name ?? 'Choose a project'}</span>
              <span class="path-hint">{activeProject?.path ?? 'No project selected'}</span>
            </span>
            {activeUnread > 0 ? (
              <span class="workbench-project-unread" aria-hidden="true">
                {activeUnread > 99 ? '99+' : activeUnread}
              </span>
            ) : null}
            <ChevronDownIcon size={13} />
          </button>
        ) : (
          <Text as="p" size="micro" tone="muted" weight="bold" class="eyebrow">
            Projects
          </Text>
        )}
        <div class={desktop ? 'side-head-actions' : 'project-switcher-actions'}>
          <IconButton
            size="sm"
            class="side-mini-btn"
            label="Add project"
            title="Add project"
            icon={<PlusIcon size={15} />}
            onClick={() => setDialog('add')}
          />
          <ProjectOptionsMenu
            activeProject={activeProject}
            menu={menu}
            onRename={() => setDialog('rename')}
            onDelete={() => setDialog('delete')}
          />
          {!desktop || projectsOpen ? (
            <IconButton
              size="sm"
              class="side-mini-btn"
              label={desktop ? 'Close project switcher' : 'Close sessions panel'}
              data-autofocus={desktop ? undefined : true}
              icon={<CloseIcon size={15} />}
              onClick={desktop ? () => closeProjects() : actions.closeSheets}
            />
          ) : null}
        </div>

        {/* The dropdown lives inside the trigger's row so that row is its
         * containing block: it hangs under the trigger, out of flow, and the rail
         * below keeps its geometry instead of being pushed down. */}
        {desktop && projectsOpen ? (
          <div
            id={disclosureId}
            class="workbench-projects-disclosure"
            aria-label="Available projects"
            data-escape-local
            onMouseEnter={holdProjects}
            onMouseLeave={hoverCloseProjects}
            // Tabbing out must take the overlay with it: the rail's next control
            // (Control plane, New session) sits behind this panel, and leaving
            // focus in a list that covers it would hide where the reader landed.
            onKeyDown={(event) => {
              if (event.key === 'Tab') closeProjects(false);
            }}
          >
            {projectList}
          </div>
        ) : null}
      </div>

      {/* A phone keeps the list in the sheet: there is no trigger to hover, and the
       * sheet is the only place projects exist there. */}
      {!desktop ? <div>{projectList}</div> : null}

      {desktop ? (
        <button
          type="button"
          class="workbench-control-plane"
          aria-label="Open control plane"
          aria-pressed={controlPlaneOpen.value}
          onClick={() => {
            closeProjects(false);
            controlPlaneOpen.value = true;
          }}
        >
          <span class="workbench-control-mark" aria-hidden="true">k.</span>
          <span>Control plane</span>
        </button>
      ) : null}

      <AddProjectDialog open={dialog === 'add'} onClose={() => setDialog(null)} />
      <RenameProjectDialog
        project={activeProject}
        open={dialog === 'rename'}
        onClose={() => setDialog(null)}
      />
      <DeleteProjectDialog
        project={activeProject}
        open={dialog === 'delete'}
        onClose={() => setDialog(null)}
      />
    </div>
  );
}
