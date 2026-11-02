import { useEffect, useId, useRef, useState } from 'preact/hooks';
import type { JSX } from 'preact';
import type { Project } from '../api/types';
import { modKey } from '../lib/platform';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Dialog } from './Dialog';
import {
  AgentsIcon,
  EditIcon,
  FileCodeIcon,
  FileIcon,
  FolderIcon,
  GitIcon,
  KebabIcon,
  PaletteIcon,
  PlusIcon,
  SearchIcon,
  TerminalIcon,
} from './icons';
import '../styles/rail.css';

/**
 * Discord-style initials: first letter of each word, at most two characters.
 * Falls back to the last path segment so path-only projects still get a glyph.
 */
export function projectInitials(name: string, path = ''): string {
  const words = name
    .trim()
    .split(/[\s\-_./\\]+/)
    .filter((word) => word.length > 0);
  const source = words.length > 0 ? words : path.split('/').filter((part) => part.length > 0).slice(-1);
  const letters = source
    .slice(0, 2)
    .map((word) => Array.from(word)[0] ?? '')
    .join('');
  return (letters === '' ? '?' : letters).toUpperCase();
}

/**
 * The square glyph for one project: the project's own `favicon.ico` when it has
 * one, otherwise its initials.
 *
 * The icon is an object URL owned by `state/projectIcons.ts` (the bytes were
 * fetched with the bearer token, which an `<img src>` could not send). It is
 * decorative either way: the button around it carries the accessible name, so the
 * image is `aria-hidden`.
 */
function ProjectGlyph({ project }: { project: Project }): JSX.Element {
  const iconUrl = store.projectIcons.value[project.id];
  const initials = projectInitials(project.name, project.path);
  if (!iconUrl) {
    return (
      <span class="project-rail-avatar" aria-hidden="true">
        {initials}
      </span>
    );
  }
  return (
    <span class="project-rail-avatar has-icon" aria-hidden="true">
      <img class="project-rail-icon" src={iconUrl} alt="" draggable={false} decoding="async" />
    </span>
  );
}

/**
 * The glyph for one tool name, as the sessions rail draws it.
 *
 * The mock puts up to three small symbols after a session's title so a glance
 * says what the session *was*: shell work, edits, reads, searches. Anything
 * unrecognised gets a generic file mark rather than nothing, so a plugin or MCP
 * tool still reads as "this session called tools".
 */
export function SessionGlyph({ kind }: { kind: string }): JSX.Element {
  const name = kind.replace(/^plugin__[^_]+__/, '');
  if (name === 'run_command' || name === 'terminal') return <TerminalIcon size={10} />;
  if (name === 'edit_file' || name === 'write_file' || name === 'apply_patch') return <EditIcon size={10} />;
  if (name === 'read_file' || name === 'get_workspace_file') return <FileIcon size={10} />;
  if (name === 'search_files' || name === 'grep') return <SearchIcon size={10} />;
  if (name === 'list_dir' || name === 'get_workspace_tree') return <FolderIcon size={10} />;
  if (name === 'git_diff' || name === 'git_status') return <GitIcon size={10} />;
  if (name.startsWith('spawn_agent') || name === 'report_status') return <AgentsIcon size={10} />;
  return <FileCodeIcon size={10} />;
}

function sessionLabel(count: number): string {
  return count === 1 ? '1 session' : `${count} sessions`;
}

/** Absolute-path validation shared by the hint and the submit guard. */
export function pathHint(value: string): string | null {
  const path = value.trim();
  if (path === '') return 'A project path is required.';
  if (!path.startsWith('/')) return 'Use an absolute path, starting with / — for example /home/you/project.';
  return null;
}

interface AddProjectDialogProps {
  open: boolean;
  onClose: () => void;
}

/**
 * Exported so the sessions sheet can offer project management on a phone, where
 * the rail itself is hidden: the dialogs are the only way to add, rename or
 * delete a project, and they must not be reachable only by mouse.
 */
export function AddProjectDialog({ open, onClose }: AddProjectDialogProps): JSX.Element {
  const fieldId = useId();
  const [path, setPath] = useState('');
  const [name, setName] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setPath('');
    setName('');
    setError(null);
    setBusy(false);
  }, [open]);

  const submit = async (event: JSX.TargetedSubmitEvent<HTMLFormElement>): Promise<void> => {
    event.preventDefault();
    if (busy) return;
    const trimmedPath = path.trim();
    const hint = pathHint(trimmedPath);
    if (hint) {
      setError(hint);
      return;
    }
    setError(null);
    setBusy(true);
    const created = await actions.createProject(name.trim(), trimmedPath);
    setBusy(false);
    // `createProject` resolves null on failure and has already toasted the reason.
    if (!created) {
      setError('Could not add that project. Check the path exists and is readable, then try again.');
      return;
    }
    onClose();
  };

  return (
    <Dialog open={open} title="Add project" onClose={onClose} class="project-rail-dialog">
      <form class="project-rail-form" onSubmit={(event) => void submit(event)} noValidate>
        <div class="field">
          <label class="field-label" for={`${fieldId}-path`}>
            Project path
          </label>
          <input
            id={`${fieldId}-path`}
            class="input mono"
            type="text"
            value={path}
            spellcheck={false}
            autocomplete="off"
            placeholder="/home/you/project"
            aria-required="true"
            aria-invalid={error ? 'true' : undefined}
            aria-describedby={`${fieldId}-path-hint${error ? ` ${fieldId}-error` : ''}`}
            data-autofocus
            onInput={(event) => setPath(event.currentTarget.value)}
          />
          <p class="field-hint" id={`${fieldId}-path-hint`}>
            Absolute directory on the machine running kc-server. A leading ~ is not expanded.
          </p>
        </div>

        <div class="field">
          <label class="field-label" for={`${fieldId}-name`}>
            Display name (optional)
          </label>
          <input
            id={`${fieldId}-name`}
            class="input"
            type="text"
            value={name}
            maxLength={120}
            autocomplete="off"
            placeholder="Defaults to the folder name"
            onInput={(event) => setName(event.currentTarget.value)}
          />
          <p class="field-hint">Leave empty to use the folder name.</p>
        </div>

        {error ? (
          <p class="field-error" id={`${fieldId}-error`} role="alert">
            {error}
          </p>
        ) : null}

        <div class="dialog-actions">
          <button type="button" class="button" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" class="button button-primary" disabled={busy}>
            {busy ? 'Adding…' : 'Add project'}
          </button>
        </div>
      </form>
    </Dialog>
  );
}

interface RenameProjectDialogProps {
  project: Project | null;
  open: boolean;
  onClose: () => void;
}

export function RenameProjectDialog({ project, open, onClose }: RenameProjectDialogProps): JSX.Element {
  const fieldId = useId();
  const [draft, setDraft] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setDraft(project?.name ?? '');
    setError(null);
    setBusy(false);
    // Select the whole name so typing replaces it.
    window.setTimeout(() => inputRef.current?.select(), 0);
  }, [open, project?.id, project?.name]);

  const submit = async (event: JSX.TargetedSubmitEvent<HTMLFormElement>): Promise<void> => {
    event.preventDefault();
    if (!project || busy) return;
    const next = draft.trim();
    if (next === '') {
      setError('A project name is required.');
      return;
    }
    if (next === project.name) {
      onClose();
      return;
    }
    setError(null);
    setBusy(true);
    const ok = await actions.renameProjectById(project.id, next);
    setBusy(false);
    if (!ok) {
      setError('Could not rename the project. Check the server connection and try again.');
      return;
    }
    onClose();
  };

  return (
    <Dialog open={open} title="Rename project" onClose={onClose} class="project-rail-dialog">
      <form class="project-rail-form" onSubmit={(event) => void submit(event)} noValidate>
        <div class="field">
          <label class="field-label" for={`${fieldId}-name`}>
            Project name
          </label>
          <input
            id={`${fieldId}-name`}
            ref={inputRef}
            class="input"
            type="text"
            value={draft}
            maxLength={120}
            autocomplete="off"
            aria-required="true"
            aria-invalid={error ? 'true' : undefined}
            aria-describedby={error ? `${fieldId}-error` : undefined}
            data-autofocus
            onInput={(event) => setDraft(event.currentTarget.value)}
          />
          <p class="field-hint">{project ? `Showing in the rail as ${project.path}` : ''}</p>
        </div>

        {error ? (
          <p class="field-error" id={`${fieldId}-error`} role="alert">
            {error}
          </p>
        ) : null}

        <div class="dialog-actions">
          <button type="button" class="button" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" class="button button-primary" disabled={busy}>
            {busy ? 'Saving…' : 'Save name'}
          </button>
        </div>
      </form>
    </Dialog>
  );
}

interface DeleteProjectDialogProps {
  project: Project | null;
  open: boolean;
  onClose: () => void;
}

export function DeleteProjectDialog({ project, open, onClose }: DeleteProjectDialogProps): JSX.Element {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setError(null);
    setBusy(false);
  }, [open, project?.id]);

  const confirm = async (): Promise<void> => {
    if (!project || busy) return;
    setError(null);
    setBusy(true);
    // `true` forces the server-side cascade: sessions of the project go too.
    const ok = await actions.deleteProjectById(project.id, true);
    setBusy(false);
    if (!ok) {
      setError('Could not delete the project. Check the server connection and try again.');
      return;
    }
    onClose();
  };

  return (
    <Dialog open={open} title="Delete project?" onClose={onClose} class="dialog-confirm">
      {project ? (
        <div class="project-rail-delete-body">
          <p>
            Remove <strong>{project.name}</strong> from the rail?
          </p>
          <ul class="project-rail-delete-list">
            <li>
              {project.session_count > 0
                ? `Its ${sessionLabel(project.session_count)} — conversations, runs and approvals — are deleted with it.`
                : 'It has no sessions, so only the rail entry is removed.'}
            </li>
            <li>
              Files on disk are never touched: <code class="inline-path">{project.path}</code> stays exactly as it is.
            </li>
          </ul>
          {error ? (
            <p class="field-error" role="alert">
              {error}
            </p>
          ) : null}
          <div class="dialog-actions">
            <button type="button" class="button" data-autofocus onClick={onClose}>
              Cancel
            </button>
            <button type="button" class="button button-danger" disabled={busy} onClick={() => void confirm()}>
              {busy ? 'Deleting…' : 'Delete project'}
            </button>
          </div>
        </div>
      ) : null}
    </Dialog>
  );
}

type DialogKind = 'add' | 'rename' | 'delete' | null;

interface MenuState {
  projectId: string;
  top: number;
  left: number;
}

/**
 * Far-left project switcher. Reads the store directly (no props) and renders
 * one square icon per project. Layout is a plain column: the surrounding
 * layout decides where it sits, so it works both inline and on mobile.
 */
export function ProjectRail(): JSX.Element {
  const projects = store.projects.value;
  const loading = store.projectsLoading.value;
  const activeProject = store.currentProject.value;
  const activeId = activeProject?.id ?? null;

  const [dialog, setDialog] = useState<DialogKind>(null);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuTriggerRef = useRef<HTMLElement | null>(null);
  const activeButtonRef = useRef<HTMLButtonElement>(null);

  const menuProject = menu ? projects.find((project) => project.id === menu.projectId) ?? null : null;
  const renameProject = projects.find((project) => project.id === activeId) ?? null;

  const closeMenu = (restoreFocus = true): void => {
    const trigger = menuTriggerRef.current;
    setMenu(null);
    if (restoreFocus && trigger) {
      window.setTimeout(() => {
        if (trigger.isConnected) trigger.focus();
      }, 0);
    }
  };

  const openMenu = (project: Project, trigger: HTMLElement | null): void => {
    const rect = trigger?.getBoundingClientRect();
    menuTriggerRef.current = trigger;
    setMenu({
      projectId: project.id,
      top: rect && rect.height > 0 ? rect.top : 12,
      left: rect && rect.width > 0 ? rect.right + 8 : 68,
    });
  };

  useEffect(() => {
    if (!menu) return;
    const first = menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]');
    first?.focus();

    const onPointerDown = (event: MouseEvent): void => {
      const target = event.target as Node | null;
      const trigger = menuTriggerRef.current;
      // Clicking the trigger toggles the menu itself; leave it to the click handler.
      if (trigger && target && trigger.contains(target)) return;
      if (menuRef.current?.contains(target)) return;
      const hadFocus = Boolean(menuRef.current?.contains(document.activeElement));
      setMenu(null);
      if (hadFocus && trigger && trigger.isConnected) trigger.focus();
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        closeMenu();
      }
    };
    const onLoseAnchor = (): void => setMenu(null);

    document.addEventListener('mousedown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    window.addEventListener('resize', onLoseAnchor);
    window.addEventListener('scroll', onLoseAnchor, true);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('resize', onLoseAnchor);
      window.removeEventListener('scroll', onLoseAnchor, true);
    };
  }, [menu]);

  // A deleted or switched-away project must not leave a stale menu open.
  useEffect(() => {
    if (menu && !projects.some((project) => project.id === menu.projectId)) setMenu(null);
  }, [menu, projects]);

  // Long project lists scroll; keep the current project's square in view.
  useEffect(() => {
    const button = activeButtonRef.current;
    if (button && typeof button.scrollIntoView === 'function') {
      button.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    }
  }, [activeId, projects.length]);

  const closeDialog = (): void => setDialog(null);

  return (
    <>
      <nav class="project-rail" aria-label="Projects" data-testid="project-rail">
        <div class="project-rail-brand">
          <span class="brand-mark" title="karucode" aria-hidden="true">
            k<span>.</span>
          </span>
        </div>
        <div class="project-rail-divider" aria-hidden="true" />

        <div class="project-rail-scroll">
          {loading && projects.length === 0 ? (
            <div class="project-rail-loading" aria-busy="true" aria-label="Loading projects">
              {[0, 1, 2].map((index) => (
                <span class="skeleton project-rail-skeleton" key={index} />
              ))}
            </div>
          ) : null}

          {!loading && projects.length === 0 ? (
            <p class="project-rail-empty">No projects</p>
          ) : null}

          <ul class="project-rail-list" aria-busy={loading || undefined}>
            {projects.map((project) => {
              const active = project.id === activeId;
              const sessions = sessionLabel(project.session_count);
              const tooltip =
                project.session_count > 0
                  ? `${project.name}\n${project.path}\n${sessions}`
                  : `${project.name}\n${project.path}`;
              return (
                <li
                  class={`project-rail-item-wrap${active ? ' is-active' : ''}`}
                  key={project.id}
                  data-project-id={project.id}
                >
                  <button
                    type="button"
                    ref={active ? activeButtonRef : undefined}
                    class={`project-rail-item${active ? ' is-active' : ''}`}
                    aria-label={`Switch to ${project.name} (${project.path})`}
                    aria-current={active ? 'true' : undefined}
                    title={tooltip}
                    onClick={() => {
                      if (!active) void actions.switchProject(project.id);
                    }}
                    onContextMenu={(event) => {
                      if (!active) return;
                      event.preventDefault();
                      openMenu(project, event.currentTarget);
                    }}
                  >
                    <ProjectGlyph project={project} />
                  </button>

                  {active ? (
                    <button
                      type="button"
                      class="project-rail-kebab"
                      aria-label={`Project actions for ${project.name}`}
                      aria-haspopup="menu"
                      aria-expanded={menu?.projectId === project.id}
                      title={`Project actions for ${project.name}`}
                      onClick={(event) => {
                        if (menu?.projectId === project.id) closeMenu(false);
                        else openMenu(project, event.currentTarget);
                      }}
                    >
                      <KebabIcon size={13} />
                    </button>
                  ) : null}
                </li>
              );
            })}
          </ul>

          <div class="project-rail-add-row">
            <button
              type="button"
              class="project-rail-add"
              aria-label="Add project"
              title="Add project"
              onClick={() => setDialog('add')}
            >
              <PlusIcon size={17} />
            </button>
          </div>
        </div>

        <div class="project-rail-bottom">
          <button
            type="button"
            class="rail-tool rail-tool-accent"
            data-tip={`Theme studio · ${modKey()} Shift T`}
            aria-label="Open theme studio"
            aria-haspopup="dialog"
            aria-keyshortcuts="Control+Shift+T Meta+Shift+T"
            onClick={actions.openThemeStudio}
          >
            <PaletteIcon size={15} />
          </button>
          <button
            type="button"
            class="rail-tool"
            data-tip={`Command palette · ${modKey()} K`}
            aria-label="Open command palette"
            aria-haspopup="dialog"
            aria-keyshortcuts="Control+K Meta+K"
            onClick={actions.togglePalette}
          >
            <SearchIcon size={15} />
          </button>
          <button
            type="button"
            class="rail-avatar"
            data-tip="Settings"
            aria-label="Open settings"
            aria-haspopup="dialog"
            onClick={actions.openSettings}
          >
            KC
          </button>
        </div>

        {menu && menuProject ? (
          <div
            ref={menuRef}
            class="project-rail-menu"
            role="menu"
            aria-label={`Project actions for ${menuProject.name}`}
            style={{ top: `${menu.top}px`, left: `${menu.left}px` }}
            onKeyDown={(event) => {
              const items = Array.from(
                menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [],
              );
              if (items.length === 0) return;
              const index = items.indexOf(document.activeElement as HTMLButtonElement);
              let next = index;
              if (event.key === 'ArrowDown') next = (index + 1) % items.length;
              else if (event.key === 'ArrowUp') next = (index - 1 + items.length) % items.length;
              else if (event.key === 'Home') next = 0;
              else if (event.key === 'End') next = items.length - 1;
              else if (event.key === 'Tab') {
                closeMenu(false);
                return;
              } else return;
              event.preventDefault();
              items[next]?.focus();
            }}
          >
            <div class="project-rail-menu-meta">
              <span class="project-rail-menu-name">{menuProject.name}</span>
              <span class="project-rail-menu-path">{menuProject.path}</span>
              <span class="project-rail-menu-count">
                {menuProject.session_count > 0 ? sessionLabel(menuProject.session_count) : 'No sessions'}
              </span>
            </div>
            <button
              type="button"
              role="menuitem"
              class="menu-item"
              onClick={() => {
                closeMenu(false);
                setDialog('rename');
              }}
            >
              Rename…
            </button>
            <button
              type="button"
              role="menuitem"
              class="menu-item menu-item-danger"
              onClick={() => {
                closeMenu(false);
                setDialog('delete');
              }}
            >
              Delete project…
            </button>
          </div>
        ) : null}
      </nav>

      <AddProjectDialog open={dialog === 'add'} onClose={closeDialog} />
      <RenameProjectDialog project={renameProject} open={dialog === 'rename'} onClose={closeDialog} />
      <DeleteProjectDialog project={renameProject} open={dialog === 'delete'} onClose={closeDialog} />
    </>
  );
}