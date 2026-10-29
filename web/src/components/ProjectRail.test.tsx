import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/preact';
import type { Project } from '../api/types';
import * as store from '../state/store';
import * as actions from '../state/actions';
import { ProjectRail, pathHint, projectInitials } from './ProjectRail';

vi.mock('../state/actions', () => ({
  switchProject: vi.fn(async () => undefined),
  createProject: vi.fn(async () => null),
  renameProjectById: vi.fn(async () => true),
  deleteProjectById: vi.fn(async () => true),
  openThemeStudio: vi.fn(),
  togglePalette: vi.fn(),
  openSettings: vi.fn(),
}));

const NOW = 1_710_000_100_000;

function project(overrides: Partial<Project> = {}): Project {
  return {
    id: 'p_karucode',
    name: 'KaruCode',
    path: '/home/me/karucode',
    created_at: NOW,
    updated_at: NOW,
    session_count: 0,
    ...overrides,
  };
}

const mainProject = project();
const docsProject = project({
  id: 'p_docs',
  name: 'Docs Site',
  path: '/home/me/docs',
  session_count: 3,
});

const createdProject = project({ id: 'p_new', name: 'My Project', path: '/tmp/app' });

function setProjects(projects: Project[], currentId: string | null, loading = false): void {
  store.projects.value = projects;
  store.projectsLoading.value = loading;
  store.currentProjectId.value = currentId;
  store.projectSheetOpen.value = false;
}

function renderRail() {
  return render(<ProjectRail />);
}

beforeEach(() => {
  setProjects([mainProject, docsProject], mainProject.id);
  vi.mocked(actions.switchProject).mockClear();
  vi.mocked(actions.createProject).mockClear();
  vi.mocked(actions.createProject).mockResolvedValue(null);
  vi.mocked(actions.renameProjectById).mockClear();
  vi.mocked(actions.renameProjectById).mockResolvedValue(true);
  vi.mocked(actions.deleteProjectById).mockClear();
  vi.mocked(actions.deleteProjectById).mockResolvedValue(true);
});

describe('ProjectRail', () => {
  it('renders the brand mark and the rail entry points', () => {
    const { container } = renderRail();

    expect(container.querySelector('.brand-mark')?.textContent).toBe('k.');
    expect(screen.getByRole('button', { name: 'Open theme studio' })).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Open command palette' })).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Open settings' })).not.toBeNull();
  });

  it('wires the bottom tools to the theme studio, the palette and settings', () => {
    renderRail();

    fireEvent.click(screen.getByRole('button', { name: 'Open theme studio' }));
    expect(actions.openThemeStudio).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole('button', { name: 'Open command palette' }));
    expect(actions.togglePalette).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole('button', { name: 'Open settings' }));
    expect(actions.openSettings).toHaveBeenCalledTimes(1);
  });

  it('renders one button per project with the documented accessible name', () => {
    const { container } = renderRail();

    const items = container.querySelectorAll('.project-rail-item');
    expect(items).toHaveLength(2);

    expect(
      screen.getByRole('button', { name: 'Switch to KaruCode (/home/me/karucode)' }),
    ).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Switch to Docs Site (/home/me/docs)' })).not.toBeNull();

    // Initials per project (max two characters), rendered inside the square icon.
    expect(items[0]?.querySelector('.project-rail-avatar')?.textContent).toBe('K');
    expect(items[1]?.querySelector('.project-rail-avatar')?.textContent).toBe('DS');
  });

  it('marks the active project, shows its left pill and exposes actions on it only', () => {
    const { container } = renderRail();

    const active = container.querySelectorAll('.project-rail-item.is-active');
    expect(active).toHaveLength(1);
    expect(active[0]?.getAttribute('aria-current')).toBe('true');
    expect(active[0]?.getAttribute('data-project-id')).toBeNull();
    expect(active[0]?.closest('[data-project-id]')?.getAttribute('data-project-id')).toBe(
      mainProject.id,
    );

    // Only the active row carries the three-dot menu trigger.
    const kebabs = container.querySelectorAll('.project-rail-kebab');
    expect(kebabs).toHaveLength(1);
    expect(screen.getByRole('button', { name: 'Project actions for KaruCode' })).not.toBeNull();
    expect(container.querySelector('.project-rail-item:not(.is-active)')?.getAttribute('aria-current')).toBeNull();
  });

  it('switches to a non-active project and ignores clicks on the active one', () => {
    renderRail();

    fireEvent.click(screen.getByRole('button', { name: 'Switch to Docs Site (/home/me/docs)' }));
    expect(actions.switchProject).toHaveBeenCalledWith(docsProject.id);
    expect(actions.switchProject).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole('button', { name: 'Switch to KaruCode (/home/me/karucode)' }));
    expect(actions.switchProject).toHaveBeenCalledTimes(1);
  });

  it('shows a loading state before the first project response', () => {
    setProjects([], null, true);
    renderRail();

    const loading = screen.getByLabelText('Loading projects');
    expect(loading.getAttribute('aria-busy')).toBe('true');
    expect(loading.querySelectorAll('.project-rail-skeleton').length).toBeGreaterThan(0);
    expect(screen.queryByRole('button', { name: /^Switch to / })).toBeNull();
  });

  it('shows the session count in the tooltip and in the project menu', () => {
    setProjects([mainProject, docsProject], docsProject.id);
    renderRail();

    const docs = screen.getByRole('button', { name: 'Switch to Docs Site (/home/me/docs)' });
    expect(docs.getAttribute('title')).toContain('3 sessions');

    fireEvent.click(screen.getByRole('button', { name: 'Project actions for Docs Site' }));
    const menu = screen.getByRole('menu', { name: 'Project actions for Docs Site' });
    expect(menu.textContent).toContain('3 sessions');
    expect(menu.textContent).toContain('/home/me/docs');

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(actions.switchProject).not.toHaveBeenCalled();
  });

  it('adds a project through the dialog with the name-then-path argument order', async () => {
    vi.mocked(actions.createProject).mockResolvedValue(createdProject);
    renderRail();

    fireEvent.click(screen.getByRole('button', { name: 'Add project' }));
    const dialog = screen.getByRole('dialog', { name: 'Add project' });
    expect(dialog).not.toBeNull();

    const pathInput = within(dialog).getByLabelText('Project path') as HTMLInputElement;
    const nameInput = within(dialog).getByLabelText(/Display name/) as HTMLInputElement;
    fireEvent.input(pathInput, { target: { value: '/tmp/app' } });
    fireEvent.input(nameInput, { target: { value: 'My Project' } });

    fireEvent.submit(dialog.querySelector('form') as HTMLFormElement);

    expect(actions.createProject).toHaveBeenCalledWith('My Project', '/tmp/app');

    // Dialog closes after a successful create.
    await vi.waitFor(() => expect(screen.queryByRole('dialog', { name: 'Add project' })).toBeNull());
  });

  it('rejects relative or empty paths inline without calling the API', () => {
    renderRail();

    fireEvent.click(screen.getByRole('button', { name: 'Add project' }));
    const dialog = screen.getByRole('dialog', { name: 'Add project' });
    const form = dialog.querySelector('form') as HTMLFormElement;
    const pathInput = within(dialog).getByLabelText('Project path');

    fireEvent.submit(form);
    expect(within(dialog).getByRole('alert').textContent).toContain('required');
    expect(actions.createProject).not.toHaveBeenCalled();

    fireEvent.input(pathInput, { target: { value: 'tmp/app' } });
    fireEvent.submit(form);
    expect(within(dialog).getByRole('alert').textContent).toContain('absolute');
    expect(actions.createProject).not.toHaveBeenCalled();
  });

  it('shows an inline error when the server rejects the new project', async () => {
    vi.mocked(actions.createProject).mockResolvedValue(null);
    renderRail();

    fireEvent.click(screen.getByRole('button', { name: 'Add project' }));
    const dialog = screen.getByRole('dialog', { name: 'Add project' });
    fireEvent.input(within(dialog).getByLabelText('Project path'), {
      target: { value: '/tmp/app' },
    });
    fireEvent.submit(dialog.querySelector('form') as HTMLFormElement);

    await vi.waitFor(() =>
      expect(within(dialog).getByRole('alert').textContent).toContain('Could not add that project'),
    );
    expect(screen.getByRole('dialog', { name: 'Add project' })).not.toBeNull();
  });

  it('closes the add dialog on Escape and on Cancel', () => {
    renderRail();

    fireEvent.click(screen.getByRole('button', { name: 'Add project' }));
    expect(screen.queryByRole('dialog', { name: 'Add project' })).not.toBeNull();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('dialog', { name: 'Add project' })).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Add project' }));
    const dialog = screen.getByRole('dialog', { name: 'Add project' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('dialog', { name: 'Add project' })).toBeNull();
  });

  it('renames the current project from the three-dot menu', async () => {
    renderRail();

    fireEvent.click(screen.getByRole('button', { name: 'Project actions for KaruCode' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Rename…' }));

    const dialog = screen.getByRole('dialog', { name: 'Rename project' });
    const input = within(dialog).getByLabelText('Project name') as HTMLInputElement;
    expect(input.value).toBe('KaruCode');

    fireEvent.input(input, { target: { value: 'KaruCode CLI' } });
    fireEvent.submit(dialog.querySelector('form') as HTMLFormElement);

    expect(actions.renameProjectById).toHaveBeenCalledWith(mainProject.id, 'KaruCode CLI');
    await vi.waitFor(() => expect(screen.queryByRole('dialog', { name: 'Rename project' })).toBeNull());
  });

  it('refuses an empty rename without calling the action', () => {
    renderRail();

    fireEvent.click(screen.getByRole('button', { name: 'Project actions for KaruCode' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Rename…' }));

    const dialog = screen.getByRole('dialog', { name: 'Rename project' });
    const input = within(dialog).getByLabelText('Project name') as HTMLInputElement;
    fireEvent.input(input, { target: { value: '   ' } });
    fireEvent.submit(dialog.querySelector('form') as HTMLFormElement);

    expect(within(dialog).getByRole('alert').textContent).toContain('required');
    expect(actions.renameProjectById).not.toHaveBeenCalled();
  });

  it('confirms delete, explains the cascade and calls deleteProjectById(id, true)', async () => {
    renderRail();

    fireEvent.click(screen.getByRole('button', { name: 'Project actions for KaruCode' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete project…' }));

    const dialog = screen.getByRole('dialog', { name: 'Delete project?' });
    expect(dialog.textContent).toContain('Delete project?');
    expect(dialog.textContent).toContain('Files on disk are never touched');
    expect(actions.deleteProjectById).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete project' }));

    expect(actions.deleteProjectById).toHaveBeenCalledWith(mainProject.id, true);
    await vi.waitFor(() =>
      expect(screen.queryByRole('dialog', { name: 'Delete project?' })).toBeNull(),
    );
  });

  it('warns that sessions are deleted too when the project has sessions', () => {
    setProjects([mainProject, docsProject], docsProject.id);
    renderRail();

    fireEvent.click(screen.getByRole('button', { name: 'Project actions for Docs Site' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete project…' }));

    const dialog = screen.getByRole('dialog', { name: 'Delete project?' });
    expect(dialog.textContent).toContain('3 sessions');
    expect(dialog.textContent).toContain('never touched');
  });

  it('opens project actions from a right-click on the active item', () => {
    renderRail();

    const activeItem = screen.getByRole('button', { name: 'Switch to KaruCode (/home/me/karucode)' });
    fireEvent.contextMenu(activeItem);
    expect(screen.getByRole('menu', { name: 'Project actions for KaruCode' })).not.toBeNull();
  });
});

describe('ProjectRail helpers', () => {
  it('derives at most two initials from the project name', () => {
    expect(projectInitials('KaruCode')).toBe('K');
    expect(projectInitials('Docs Site')).toBe('DS');
    expect(projectInitials('my-cool_api.service')).toBe('MC');
    expect(projectInitials('multi word project name')).toBe('MW');
    expect(projectInitials('', '/home/me/karucode')).toBe('K');
    expect(projectInitials('   ')).toBe('?');
  });

  it('renders every project glyph in the neutral rail tile, with no per-project tint', () => {
    renderRail();

    const active = screen.getByRole('button', { name: 'Switch to KaruCode (/home/me/karucode)' });
    const idle = screen.getByRole('button', { name: 'Switch to Docs Site (/home/me/docs)' });
    // The mock's tiles are all the same grey square; only the active one carries gold.
    expect(active.querySelector('.project-rail-avatar')?.textContent).toBe('K');
    expect(idle.querySelector('.project-rail-avatar')?.textContent).toBe('DS');
    expect(active.parentElement?.hasAttribute('data-tone')).toBe(false);
    expect(idle.parentElement?.hasAttribute('data-tone')).toBe(false);
  });

  it('classifies paths for the inline hint', () => {
    expect(pathHint('')).toContain('required');
    expect(pathHint('   ')).toContain('required');
    expect(pathHint('relative/deep/path')).toContain('absolute');
    expect(pathHint('~/project')).toContain('absolute');
    expect(pathHint('/home/me/project')).toBeNull();
  });
});