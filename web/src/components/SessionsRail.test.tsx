import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/preact';
import { SessionList, SessionsRail, sessionGroup, type SessionListProps } from './SessionsRail';
import { secondSummaryFixture, summaryFixture, NOW } from '../test/fixtures';
import * as actions from '../state/actions';
import * as store from '../state/store';
import * as sessionFacts from '../state/sessionFacts';

function renderList(overrides: Partial<SessionListProps> = {}) {
  const props: SessionListProps = {
    sessions: [summaryFixture, secondSummaryFixture],
    currentId: 's_ab12',
    now: NOW,
    loading: false,
    workspace: '/home/me/project',
    onCreate: vi.fn(),
    onSelect: vi.fn(),
    onRename: vi.fn(),
    onDelete: vi.fn(),
    ...overrides,
  };
  const result = render(<SessionList {...props} />);
  return { ...result, props };
}

describe('SessionList', () => {
  it('renders titles, relative times, message counts, status dots and the active session', () => {
    const { container } = renderList();

    expect(screen.getByText('Fix login bug')).not.toBeNull();
    expect(screen.getByText('Add workspace tree')).not.toBeNull();
    expect(screen.getByText('2m ago')).not.toBeNull();
    expect(screen.getByText('12 messages')).not.toBeNull();
    expect(screen.getByText('3 messages')).not.toBeNull();
    // Meta is separated by the mock's 3px dot, not a middot glyph.
    expect(container.querySelector('.session-meta .meta-dot')).not.toBeNull();
    // The group heading carries its own count, like the mock's `RECENT SESSIONS 2`.
    expect(container.querySelector('.session-group-label span')?.textContent).toBe('2');

    const rows = container.querySelectorAll('.session-row');
    expect(rows).toHaveLength(2);
    expect(rows[0]?.querySelector('.session-open')?.getAttribute('aria-current')).toBe('true');
    expect(rows[1]?.querySelector('.session-open')?.getAttribute('aria-current')).toBeNull();
    expect(rows[1]?.querySelector('.status-dot.status-danger')).not.toBeNull();
  });

  it('shows a running dot with screen-reader text', () => {
    const { container } = renderList({
      sessions: [{ ...summaryFixture, running: true, last_run_status: 'running' }],
    });
    expect(container.querySelector('.status-dot.status-running')).not.toBeNull();
    expect(screen.getByText('Running.')).not.toBeNull();
  });

  it('selects a session when clicked', () => {
    const { props } = renderList();
    fireEvent.click(screen.getByText('Add workspace tree'));
    expect(props.onSelect).toHaveBeenCalledWith('s_cd34');
  });

  it('renames inline on double-click and commits on blur', () => {
    const { props } = renderList();
    fireEvent.dblClick(screen.getByText('Fix login bug'));
    const input = screen.getByLabelText(/Rename session/) as HTMLInputElement;
    expect(input.value).toBe('Fix login bug');
    fireEvent.input(input, { target: { value: 'Renamed session' } });
    fireEvent.blur(input);
    expect(props.onRename).toHaveBeenCalledWith('s_ab12', 'Renamed session');
  });

  it('cancels inline rename on Escape', () => {
    const { props } = renderList();
    fireEvent.dblClick(screen.getByText('Fix login bug'));
    const input = screen.getByLabelText(/Rename session/);
    fireEvent.input(input, { target: { value: 'Discard this' } });
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(props.onRename).not.toHaveBeenCalled();
  });

  it('asks for confirmation before deleting', () => {
    const { props } = renderList();
    fireEvent.click(screen.getByRole('button', { name: 'Actions for Fix login bug' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete…' }));

    const dialog = screen.getByRole('dialog');
    expect(dialog.textContent).toContain('Delete session?');
    expect(props.onDelete).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Delete session' }));
    expect(props.onDelete).toHaveBeenCalledWith('s_ab12');
  });

  it('shows an explanatory empty state with a working create button', () => {
    const { props } = renderList({ sessions: [], currentId: null });
    expect(screen.getByText('No sessions yet')).not.toBeNull();
    expect(screen.getByText(/A session keeps one conversation/)).not.toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /New session/ }));
    expect(props.onCreate).toHaveBeenCalledTimes(1);
  });

  it('shows loading skeletons before the first response', () => {
    const { container } = renderList({ sessions: [], loading: true });
    expect(container.querySelectorAll('.skeleton-session').length).toBeGreaterThan(0);
    expect(screen.getByLabelText('Loading sessions')).not.toBeNull();
  });
});
describe('session grouping and search', () => {
  it('buckets sessions by last activity', () => {
    expect(sessionGroup(NOW - 60_000, NOW)).toBe('Today');
    expect(sessionGroup(NOW - 26 * 3_600_000, NOW)).toBe('Yesterday');
    expect(sessionGroup(NOW - 3 * 86_400_000, NOW)).toBe('Previous 7 days');
    expect(sessionGroup(NOW - 30 * 86_400_000, NOW)).toBe('Older');
  });

  it('renders date group headings above the rows', () => {
    const older = { ...secondSummaryFixture, id: 's_old', title: 'Ancient history', updated_at: NOW - 30 * 86_400_000 };
    renderList({ sessions: [summaryFixture, older] });

    expect(screen.getByRole('region', { name: 'Today sessions' })).not.toBeNull();
    expect(screen.getByRole('region', { name: 'Older sessions' })).not.toBeNull();
  });

  it('filters by title and keeps a real empty state when nothing matches', () => {
    const { rerender, props } = renderList();
    rerender(<SessionList {...props} query="workspace" />);
    expect(screen.getByText('Add workspace tree')).not.toBeNull();
    expect(screen.queryByText('Fix login bug')).toBeNull();

    rerender(<SessionList {...props} query="zzz" />);
    expect(screen.getByText('No matching sessions')).not.toBeNull();
    expect(screen.queryByRole('button', { name: /Fix login bug/ })).toBeNull();
  });
});

describe('SessionsRail shell', () => {
  function seedStore(): void {
    store.sessions.value = [summaryFixture, secondSummaryFixture];
    store.sessionsLoading.value = false;
    store.currentSessionId.value = summaryFixture.id;
    store.currentProjectId.value = 'p_karucode';
    store.projects.value = [
      {
        id: 'p_karucode',
        name: 'KaruCode',
        path: '/home/me/karucode',
        created_at: NOW,
        updated_at: NOW,
        session_count: 2,
      },
    ];
    store.clock.value = NOW;
    store.narrowViewport.value = false;
    store.mobileSheet.value = null;
  }

  beforeEach(seedStore);

  it('loads fingerprints without an idle-render cache reset', async () => {
    const load = vi.spyOn(sessionFacts, 'loadFactsFor').mockResolvedValue(undefined);
    const invalidate = vi.spyOn(sessionFacts, 'invalidateFacts');
    render(<SessionsRail />);
    await vi.waitFor(() => expect(load).toHaveBeenCalledWith(['s_ab12', 's_cd34']));
    expect(invalidate).toHaveBeenCalledWith();
    load.mockRestore();
    invalidate.mockRestore();
  });

  it('shows the workspace name, path and a platform-correct shortcut', () => {
    render(<SessionsRail />);

    // The mock's `.ws` row: a glyph tile, the project name over its path. The
    // "Your workspace" eyebrow above it was a second label for the same thing.
    expect(screen.getByText('KaruCode')).not.toBeNull();
    expect(screen.getByText('/home/me/karucode')).not.toBeNull();
    const newSession = screen.getByRole('button', { name: /New session/ });
    expect(newSession.textContent).toContain('New session');
    expect(newSession.textContent).toContain('N');
  });

  it('filters the rail from the search field', () => {
    render(<SessionsRail />);

    const search = screen.getByLabelText('Find a session') as HTMLInputElement;
    fireEvent.input(search, { target: { value: 'login' } });

    expect(screen.getByText('Fix login bug')).not.toBeNull();
    expect(screen.queryByText('Add workspace tree')).toBeNull();

    fireEvent.keyDown(search, { key: 'Escape' });
    expect(search.value).toBe('');
    expect(screen.getByText('Add workspace tree')).not.toBeNull();
  });

  it('focuses the session search on "/" and ignores the key inside inputs', async () => {
    render(<SessionsRail />);
    const search = screen.getByLabelText('Find a session') as HTMLInputElement;

    fireEvent.keyDown(window, { key: '/' });
    await vi.waitFor(() => expect(document.activeElement).toBe(search));

    // Typing "/" inside the field must not be swallowed.
    const event = new KeyboardEvent('keydown', { key: '/', bubbles: true, cancelable: true });
    search.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
  });

  it('offers real project actions in the options menu', () => {
    const refresh = vi.spyOn(actions, 'refreshSessions').mockResolvedValue(undefined);
    const settings = vi.spyOn(actions, 'openSettings').mockImplementation(() => undefined);
    render(<SessionsRail />);

    fireEvent.click(screen.getByRole('button', { name: 'Project options' }));
    const menu = screen.getByRole('menu', { name: /Project options for KaruCode/ });
    expect(menu.textContent).toContain('/home/me/karucode');

    fireEvent.click(screen.getByRole('menuitem', { name: 'Refresh sessions' }));
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('menu')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Project options' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Project settings…' }));
    expect(settings).toHaveBeenCalledTimes(1);

    refresh.mockRestore();
    settings.mockRestore();
  });

  it('creates a session from the rail button', () => {
    const create = vi.spyOn(actions, 'createSession').mockResolvedValue(null);
    render(<SessionsRail />);

    fireEvent.click(screen.getByRole('button', { name: /New session/ }));
    expect(create).toHaveBeenCalledTimes(1);
    create.mockRestore();
  });

  it('wraps the search input in the field box so the whole row is the target', () => {
    const { container } = render(<SessionsRail />);
    const search = screen.getByLabelText('Find a session') as HTMLInputElement;
    // The wrapper is the mock's `<label class="side-search">`; implicit labelling
    // means a click anywhere in the box (icon, keycap, padding) focuses the input.
    const field = container.querySelector('label.side-search');
    expect(field).not.toBeNull();
    expect(field?.querySelector('input')).toBe(search);
    expect(field?.querySelector('.keycap')).not.toBeNull();
  });

  it('keeps the project options button pinned to the right gutter', () => {
    const { container } = render(<SessionsRail />);
    const heading = container.querySelector('.project-heading');
    // The mock's heading is a space-between row; the actions sit in their own
    // group so the ⋯ never hugs the title.
    expect(heading?.querySelector('.project-title')).not.toBeNull();
    expect(heading?.querySelector('.side-head-actions .side-mini-btn')).not.toBeNull();
    expect(heading?.lastElementChild?.className).toBe('side-head-actions');
  });

  it('puts the version in the footer strip and the provider beside Settings', () => {
    store.health.value = {
      status: 'ok',
      version: '0.2.4',
      uptime_secs: 12,
      workspace: '/home/me/karucode',
      projects: [],
      auth_required: false,
      provider_configured: true,
      db: 'ok',
    };
    const { container } = render(<SessionsRail />);

    // Mock: `All systems operational … v0.2.4`, then `Settings … Connected to gateway`.
    expect(container.querySelector('.online-row .right')?.textContent).toBe('v0.2.4');
    expect(container.querySelector('.settings-row .provider')?.textContent).not.toBe('v0.2.4');
    expect(container.querySelector('.settings-row .provider')?.textContent).not.toBe('');
  });
});

describe('SessionsRail project switcher (phones)', () => {
  function seedProjects(): void {
    store.sessions.value = [summaryFixture];
    store.sessionsLoading.value = false;
    store.currentSessionId.value = summaryFixture.id;
    store.currentProjectId.value = 'p_karucode';
    store.projects.value = [
      {
        id: 'p_karucode',
        name: 'KaruCode',
        path: '/home/me/karucode',
        created_at: NOW,
        updated_at: NOW,
        session_count: 2,
      },
      {
        id: 'p_docs',
        name: 'Docs Site',
        path: '/home/me/docs',
        created_at: NOW,
        updated_at: NOW,
        session_count: 3,
      },
    ];
    store.clock.value = NOW;
    store.narrowViewport.value = true;
    store.mobileSheet.value = 'sessions';
  }

  beforeEach(seedProjects);

  it('replaces the hidden rail on a phone and lists every project', () => {
    // The rail is `display: none` below 1023px, so without this strip a phone
    // could not change project at all.
    const { container } = render(<SessionsRail />);
    const switcher = container.querySelector('.project-switcher');
    expect(switcher).not.toBeNull();

    const items = container.querySelectorAll('.project-switcher-item');
    expect(items).toHaveLength(2);
    expect(items[0]?.getAttribute('aria-label')).toBe(
      'Switch to KaruCode (/home/me/karucode)',
    );
    // The active project is marked, and is not itself a way to switch.
    expect(items[0]?.className).toContain('is-active');
    expect(items[0]?.getAttribute('aria-current')).toBe('true');
    expect(items[1]?.getAttribute('aria-current')).toBeNull();
  });

  it('is not rendered on a wide viewport, where the rail does the job', () => {
    store.narrowViewport.value = false;
    store.mobileSheet.value = null;
    const { container } = render(<SessionsRail />);
    expect(container.querySelector('.project-switcher')).toBeNull();
  });

  it('switches project from a pill and ignores a click on the active one', () => {
    const switchProject = vi.spyOn(actions, 'switchProject').mockResolvedValue(undefined);
    const { container } = render(<SessionsRail />);
    const items = container.querySelectorAll<HTMLButtonElement>('.project-switcher-item');

    fireEvent.click(items[0]!);
    expect(switchProject).not.toHaveBeenCalled();

    fireEvent.click(items[1]!);
    expect(switchProject).toHaveBeenCalledWith('p_docs');
    switchProject.mockRestore();
  });

  it('carries the project actions that used to live only on the rail', () => {
    // Add, rename and delete were reachable only from the project rail's menus,
    // which a phone does not render.
    const { container } = render(<SessionsRail />);
    expect(screen.getByRole('button', { name: 'Add project' })).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Project options' }));
    const menu = screen.getByRole('menu', { name: /Project options for KaruCode/ });
    expect(menu.textContent).toContain('/home/me/karucode');
    for (const label of ['Copy project path', 'Refresh sessions', 'Project settings…', 'Rename…', 'Delete project…']) {
      expect(within(menu).getByRole('menuitem', { name: label })).not.toBeNull();
    }

    fireEvent.click(within(menu).getByRole('menuitem', { name: 'Rename…' }));
    expect(screen.getByRole('dialog', { name: 'Rename project' })).not.toBeNull();
    expect(container.querySelector('.project-switcher')).not.toBeNull();
  });

  it('opens the add-project dialog from the switcher', () => {
    render(<SessionsRail />);
    fireEvent.click(screen.getByRole('button', { name: 'Add project' }));
    expect(screen.getByRole('dialog', { name: 'Add project' })).not.toBeNull();
  });

  it('offers the close button the toolbar used to own', () => {
    // The sessions tab in the deleted mobile toolbar was the way out of this
    // sheet; the sheet has to be able to close itself now.
    const close = vi.spyOn(actions, 'closeSheets').mockImplementation(() => undefined);
    render(<SessionsRail />);
    fireEvent.click(screen.getByRole('button', { name: 'Close sessions panel' }));
    expect(close).toHaveBeenCalledTimes(1);
    close.mockRestore();
  });
});
