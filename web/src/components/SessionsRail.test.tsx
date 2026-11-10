import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/preact';
import type { SessionSummary } from '../api/types';
import { SessionList, SessionsRail, sessionGroup, type SessionListProps } from './SessionsRail';
import { Dialog } from './Dialog';
import { secondSummaryFixture, summaryFixture, NOW } from '../test/fixtures';
import { sessionStatusView } from '../state/selectors';
import * as actions from '../state/actions';
import * as store from '../state/store';
import * as sessionFacts from '../state/sessionFacts';
import * as railPeek from '../state/railPeek';
import { controlPlaneOpen } from '../state/controlPlane';

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

  it('reads every state word and colour from the shared session status view', () => {
    // `sessionStatusView` is what the control plane reads too. Asserting the row
    // against it — rather than against literals — is what stops the two lists
    // drifting apart: a rename in the helper moves both surfaces at once.
    const states: Array<Pick<SessionSummary, 'running' | 'last_run_status'>> = [
      { running: true, last_run_status: 'running' },
      { running: false, last_run_status: 'failed' },
      { running: false, last_run_status: 'interrupted' },
      { running: false, last_run_status: 'completed' },
      { running: false, last_run_status: 'cancelled' },
      { running: false, last_run_status: null },
    ];
    for (const state of states) {
      const view = sessionStatusView(state);
      const { container, unmount } = renderList({
        sessions: [{ ...summaryFixture, ...state }],
      });
      const dot = container.querySelector('.status-dot');
      const word = container.querySelector('.session-state');
      if (view.tone === null) {
        expect(dot, JSON.stringify(state)).toBeNull();
        expect(word, JSON.stringify(state)).toBeNull();
      } else {
        expect(dot?.classList.contains(`status-${view.tone}`), JSON.stringify(state)).toBe(true);
        expect(word?.classList.contains(`session-state-${view.tone}`), JSON.stringify(state)).toBe(
          true,
        );
        expect(word?.textContent, JSON.stringify(state)).toBe(view.label);
      }
      unmount();
    }
  });

  it('says "Interrupted" for an interrupted run, in the warn tone', () => {
    const { container } = renderList({
      sessions: [{ ...summaryFixture, running: false, last_run_status: 'interrupted' }],
    });
    expect(container.querySelector('.status-dot.status-warn')).not.toBeNull();
    expect(container.querySelector('.session-state-warn')?.textContent).toBe('Interrupted');
    expect(screen.getByText('Last run interrupted.')).not.toBeNull();
  });

  it('keeps working details discoverable without repeating tool glyphs in every row', () => {
    sessionFacts.sessionFacts.value = {
      [summaryFixture.id]: { turns: 4, calls: 7, tools: ['run_command', 'edit_file'] },
    };
    const { container } = renderList({ sessions: [summaryFixture] });
    const open = container.querySelector<HTMLButtonElement>('.session-open');

    expect(open?.title).toContain('4 turns');
    expect(open?.title).toContain('7 tool calls');
    expect(open?.title).toContain('Tools: run_command, edit_file');
    expect(container.querySelector('.session-glyphs')).toBeNull();
    expect(container.querySelector('.session-turns')).toBeNull();
    expect(screen.getByText('12 messages')).not.toBeNull();
    sessionFacts.sessionFacts.value = {};
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
    fireEvent.focusOut(input);
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

  it('restores the session menu trigger when the delete child is cancelled', async () => {
    renderList();
    const trigger = screen.getByRole('button', { name: 'Actions for Fix login bug' });
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete…' }));
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    await vi.waitFor(() => expect(document.activeElement).toBe(cancel));
    fireEvent.click(cancel);
    await vi.waitFor(() => expect(screen.queryByRole('dialog', { name: 'Delete session?' })).toBeNull());
    await vi.waitFor(() => expect(document.activeElement).toBe(trigger));
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

  it('restores Project options after the settings child closes', async () => {
    function Harness() {
      return <>
        <SessionsRail />
        <Dialog
          open={store.settingsOpen.value}
          title="Settings"
          onClose={actions.closeSettings}
        >
          <button type="button" data-autofocus onClick={actions.closeSettings}>Done</button>
        </Dialog>
      </>;
    }
    store.settingsOpen.value = false;
    render(<Harness />);
    const trigger = screen.getByRole('button', { name: 'Project options' });
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Project settings…' }));
    const done = await screen.findByRole('button', { name: 'Done' });
    await vi.waitFor(() => expect(document.activeElement).toBe(done));
    fireEvent.click(done);
    await vi.waitFor(() => expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull());
    expect(document.activeElement).toBe(trigger);
  });

  it('starts a draft from the rail button', () => {
    const draft = vi.spyOn(actions, 'startDraftSession').mockImplementation(() => {});
    render(<SessionsRail />);

    fireEvent.click(screen.getByRole('button', { name: /New session/ }));
    expect(draft).toHaveBeenCalledTimes(1);
    draft.mockRestore();
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

  it('offers the newer build beside the version, and says restart once it is installed', () => {
    store.health.value = {
      status: 'ok',
      version: '107',
      build: 107,
      commit: 'abf0a42',
      commit_date: '2026-05-30T18:04:11+00:00',
      dirty: false,
      uptime_secs: 12,
      workspace: '/home/me/karucode',
      projects: [],
      auth_required: false,
      provider_configured: true,
      db: 'ok',
    };
    const update = {
      current: {
        version: '107',
        build: 107,
        commit: 'abf0a42',
        commit_date: '2026-05-30T18:04:11+00:00',
        dirty: false,
        channel: 'stable',
        target: 'x86_64-unknown-linux-gnu',
      },
      install: {
        kind: 'managed' as const,
        path: '/usr/local/bin/karucode',
        writable: false,
        detail: 'not writable',
      },
      check: {
        enabled: true,
        manifest_url: 'https://example.com/updates.json',
        interval_secs: 21600,
        checked_at: NOW,
        error: null,
      },
      latest: {
        build: 108,
        version: '108',
        commit: '9c1de77',
        commit_date: '2026-05-31T09:12:40+00:00',
        released_at: null,
        notes: null,
        url: null,
        artifact: null,
        has_target_for_this_build: true,
      },
      update_available: true,
      phase: 'idle' as const,
      staged: null,
      last_applied: null,
      restart: { supervised: true, apply_command: 'sudo karucode update --apply' },
    };
    store.updateStatus.value = update;
    const first = render(<SessionsRail />);
    expect(screen.getByText('v107')).toBeTruthy();
    const badge = screen.getByRole('button', { name: 'Update available: build 108' });
    fireEvent.click(badge);
    // The chip is a door into the panel that can do something about it.
    expect(store.settingsOpen.value).toBe(true);
    expect(store.settingsInitialTab.value).toBe('updates');
    first.unmount();

    // The same build on disk is a restart, not another install.
    store.updateStatus.value = {
      ...update,
      last_applied: {
        from: '107',
        to: '108',
        path: '/usr/local/bin/karucode',
        applied_at: NOW,
      },
    };
    render(<SessionsRail />);
    expect(screen.getByRole('button', { name: 'Restart to run build 108' })).toBeTruthy();

    // This describe seeds the store itself rather than calling resetAppState, so
    // the badge and the opened sheet are cleared here or they leak into the test
    // that asserts the footer's exact text.
    store.updateStatus.value = null;
    store.settingsInitialTab.value = null;
    store.settingsOpen.value = false;
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

describe('SessionsRail desktop project navigation', () => {
  const docsProject = {
    id: 'p_docs',
    name: 'Docs Site',
    path: '/home/me/docs',
    created_at: NOW,
    updated_at: NOW,
    session_count: 3,
  };

  beforeEach(() => {
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
      docsProject,
    ];
    store.clock.value = NOW;
    store.narrowViewport.value = false;
    store.mobileSheet.value = null;
    store.railCollapsed.value = false;
    store.notifications.value = [];
    store.projectIcons.value = {};
    controlPlaneOpen.value = false;
  });

  it('switches projects through the disclosure and retains favicons and unread indicators', async () => {
    const switchProject = vi.spyOn(actions, 'switchProject').mockResolvedValue(undefined);
    store.projectIcons.value = { p_karucode: 'blob:project-icon' };
    store.notifications.value = [
      {
        id: 'docs-one',
        kind: 'run_finished',
        title: 'Docs finished',
        body: '',
        sessionId: 's_docs_one',
        sessionTitle: 'Docs',
        projectId: 'p_docs',
        source: 'runtime',
        at: NOW,
        read: false,
      },
      {
        id: 'docs-two',
        kind: 'needs_input',
        title: 'Docs needs input',
        body: '',
        sessionId: 's_docs_two',
        sessionTitle: 'Docs',
        projectId: 'p_docs',
        source: 'runtime',
        at: NOW,
        read: false,
      },
    ];
    const { container } = render(<SessionsRail />);
    const disclosure = screen.getByRole('button', { name: 'Projects: KaruCode' });

    expect(disclosure.querySelector('img.project-rail-icon')?.getAttribute('src')).toBe(
      'blob:project-icon',
    );
    expect(container.querySelector('.project-switcher-list')).toBeNull();
    fireEvent.click(disclosure);

    const docs = screen.getByRole('button', {
      name: 'Switch to Docs Site (/home/me/docs), 2 unread notifications',
    });
    expect(within(docs).getByText('2')).not.toBeNull();
    await vi.waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: 'Switch to KaruCode (/home/me/karucode)' }),
      ),
    );
    fireEvent.click(docs);

    expect(switchProject).toHaveBeenCalledWith('p_docs');
    expect(screen.queryByLabelText('Available projects')).toBeNull();
    await vi.waitFor(() => expect(document.activeElement).toBe(disclosure));

    fireEvent.click(screen.getByRole('button', { name: 'Open control plane' }));
    expect(controlPlaneOpen.value).toBe(true);
    switchProject.mockRestore();
  });

  it('offers every retained project command in the desktop options menu', async () => {
    const refresh = vi.spyOn(actions, 'refreshSessions').mockResolvedValue(undefined);
    const settings = vi.spyOn(actions, 'openSettings').mockImplementation(() => undefined);
    const writeText = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(undefined);
    render(<SessionsRail />);

    fireEvent.click(screen.getByRole('button', { name: 'Project options' }));
    const menu = screen.getByRole('menu', { name: 'Project options for KaruCode' });
    for (const label of [
      'Copy project path',
      'Refresh sessions',
      'Project settings…',
      'Rename…',
      'Delete project…',
    ]) {
      expect(within(menu).getByRole('menuitem', { name: label })).not.toBeNull();
    }

    fireEvent.click(within(menu).getByRole('menuitem', { name: 'Copy project path' }));
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith('/home/me/karucode'));

    fireEvent.click(screen.getByRole('button', { name: 'Project options' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Refresh sessions' }));
    expect(refresh).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole('button', { name: 'Project options' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Project settings…' }));
    expect(settings).toHaveBeenCalledTimes(1);
    writeText.mockRestore();
    refresh.mockRestore();
    settings.mockRestore();
  });

  it('uses the real add, rename and delete dialogs and actions on desktop', async () => {
    const create = vi.spyOn(actions, 'createProject').mockResolvedValue({
      ...docsProject,
      id: 'p_new',
      name: 'New Project',
      path: '/tmp/new-project',
      session_count: 0,
    });
    const rename = vi.spyOn(actions, 'renameProjectById').mockResolvedValue(true);
    const remove = vi.spyOn(actions, 'deleteProjectById').mockResolvedValue(true);
    render(<SessionsRail />);

    fireEvent.click(screen.getByRole('button', { name: 'Add project' }));
    const addDialog = screen.getByRole('dialog', { name: 'Add project' });
    fireEvent.input(within(addDialog).getByLabelText('Project path'), {
      target: { value: '/tmp/new-project' },
    });
    fireEvent.input(within(addDialog).getByLabelText('Display name (optional)'), {
      target: { value: 'New Project' },
    });
    fireEvent.click(within(addDialog).getByRole('button', { name: 'Add project' }));
    await vi.waitFor(() => expect(create).toHaveBeenCalledWith('New Project', '/tmp/new-project'));
    await vi.waitFor(() => expect(screen.queryByRole('dialog', { name: 'Add project' })).toBeNull());

    fireEvent.click(screen.getByRole('button', { name: 'Project options' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Rename…' }));
    const renameDialog = screen.getByRole('dialog', { name: 'Rename project' });
    fireEvent.input(within(renameDialog).getByLabelText('Project name'), {
      target: { value: 'KaruCode Workbench' },
    });
    fireEvent.click(within(renameDialog).getByRole('button', { name: 'Save name' }));
    await vi.waitFor(() =>
      expect(rename).toHaveBeenCalledWith('p_karucode', 'KaruCode Workbench'),
    );
    await vi.waitFor(() => expect(screen.queryByRole('dialog', { name: 'Rename project' })).toBeNull());

    fireEvent.click(screen.getByRole('button', { name: 'Project options' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete project…' }));
    const deleteDialog = screen.getByRole('dialog', { name: 'Delete project?' });
    fireEvent.click(within(deleteDialog).getByRole('button', { name: 'Delete project' }));
    await vi.waitFor(() => expect(remove).toHaveBeenCalledWith('p_karucode', true));

    create.mockRestore();
    rename.mockRestore();
    remove.mockRestore();
  });

  it('makes Close dismiss the desktop disclosure without invoking the mobile action', async () => {
    const closeSheets = vi.spyOn(actions, 'closeSheets').mockImplementation(() => undefined);
    render(<SessionsRail />);
    const disclosure = screen.getByRole('button', { name: 'Projects: KaruCode' });
    fireEvent.click(disclosure);
    fireEvent.click(screen.getByRole('button', { name: 'Close project switcher' }));

    expect(screen.queryByLabelText('Available projects')).toBeNull();
    expect(closeSheets).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(document.activeElement).toBe(disclosure));
    closeSheets.mockRestore();
  });

  it('contains disclosure Escape before global run shortcuts and handles a nested menu first', async () => {
    const globalEscape = vi.fn();
    const onWindowKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') globalEscape();
    };
    window.addEventListener('keydown', onWindowKeyDown);
    render(<SessionsRail />);
    const disclosure = screen.getByRole('button', { name: 'Projects: KaruCode' });

    fireEvent.click(disclosure);
    const active = screen.getByRole('button', { name: 'Switch to KaruCode (/home/me/karucode)' });
    await vi.waitFor(() => expect(document.activeElement).toBe(active));
    fireEvent.keyDown(active, { key: 'Escape' });
    expect(screen.queryByLabelText('Available projects')).toBeNull();
    expect(globalEscape).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(document.activeElement).toBe(disclosure));

    fireEvent.click(disclosure);
    fireEvent.click(screen.getByRole('button', { name: 'Project options' }));
    const first = within(screen.getByRole('menu', { name: 'Project options for KaruCode' }))
      .getAllByRole('menuitem')[0]!;
    await vi.waitFor(() => expect(document.activeElement).toBe(first));
    fireEvent.keyDown(first, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(screen.getByLabelText('Available projects')).not.toBeNull();
    expect(globalEscape).not.toHaveBeenCalled();

    const options = screen.getByRole('button', { name: 'Project options' });
    await vi.waitFor(() => expect(document.activeElement).toBe(options));
    fireEvent.keyDown(options, { key: 'Escape' });
    expect(screen.queryByLabelText('Available projects')).toBeNull();
    expect(globalEscape).not.toHaveBeenCalled();
    window.removeEventListener('keydown', onWindowKeyDown);
  });

  it('closes on outside pointer-down but not on a pointer inside the disclosure', () => {
    render(
      <>
        <SessionsRail />
        <button type="button">Outside</button>
      </>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Projects: KaruCode' }));
    fireEvent.mouseDown(screen.getByRole('button', { name: 'Switch to Docs Site (/home/me/docs)' }));
    expect(screen.getByLabelText('Available projects')).not.toBeNull();

    fireEvent.mouseDown(screen.getByRole('button', { name: 'Outside' }));
    expect(screen.queryByLabelText('Available projects')).toBeNull();
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

  it('keeps the phone list out of the wide DOM until its desktop disclosure opens', () => {
    store.narrowViewport.value = false;
    store.mobileSheet.value = null;
    const { container } = render(<SessionsRail />);
    expect(container.querySelector('.project-switcher-desktop')).not.toBeNull();
    expect(container.querySelector('.project-switcher-list')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Projects: KaruCode' }));
    expect(container.querySelectorAll('.project-switcher-item')).toHaveLength(2);
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

  it('restores Project options when Rename closes without closing the sessions sheet', async () => {
    render(<SessionsRail />);
    const trigger = screen.getByRole('button', { name: 'Project options' });
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Rename…' }));

    const input = within(screen.getByRole('dialog', { name: 'Rename project' })).getByLabelText(
      'Project name',
    );
    await vi.waitFor(() => expect(document.activeElement).toBe(input));
    fireEvent.keyDown(input, { key: 'Escape' });

    await vi.waitFor(() => expect(screen.queryByRole('dialog', { name: 'Rename project' })).toBeNull());
    expect(screen.getByRole('dialog', { name: 'Sessions' })).not.toBeNull();
    expect(store.mobileSheet.value).toBe('sessions');
    expect(document.activeElement).toBe(trigger);
  });

  it('restores Project options when Delete closes', async () => {
    render(<SessionsRail />);
    const trigger = screen.getByRole('button', { name: 'Project options' });
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete project…' }));
    const cancel = within(screen.getByRole('dialog', { name: 'Delete project?' })).getByRole('button', { name: 'Cancel' });
    await vi.waitFor(() => expect(document.activeElement).toBe(cancel));
    fireEvent.keyDown(cancel, { key: 'Escape' });
    await vi.waitFor(() => expect(screen.queryByRole('dialog', { name: 'Delete project?' })).toBeNull());
    expect(document.activeElement).toBe(trigger);
  });

  it('opens the add-project dialog from the switcher', () => {
    render(<SessionsRail />);
    fireEvent.click(screen.getByRole('button', { name: 'Add project' }));
    expect(screen.getByRole('dialog', { name: 'Add project' })).not.toBeNull();
  });

  it('gives the project menu the desktop keyboard contract', async () => {
    render(<SessionsRail />);
    fireEvent.click(screen.getByRole('button', { name: 'Project options' }));

    const menu = screen.getByRole('menu', { name: /Project options for KaruCode/ });
    const items = within(menu).getAllByRole('menuitem');
    // Opening puts focus on the first item, like the desktop rail menu.
    await vi.waitFor(() => expect(document.activeElement).toBe(items[0]));

    fireEvent.keyDown(items[0]!, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(items[1]);
    fireEvent.keyDown(items[1]!, { key: 'End' });
    expect(document.activeElement).toBe(items[items.length - 1]);
    fireEvent.keyDown(items[items.length - 1]!, { key: 'Home' });
    expect(document.activeElement).toBe(items[0]);
    fireEvent.keyDown(items[0]!, { key: 'ArrowUp' });
    expect(document.activeElement).toBe(items[items.length - 1]);

    // Tab leaves the menu (it does not trap focus inside a popup).
    fireEvent.keyDown(items[items.length - 1]!, { key: 'Tab' });
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('closes the project menu on Escape and returns focus to its trigger', async () => {
    const closeSheets = vi.spyOn(actions, 'closeSheets').mockImplementation(() => undefined);
    render(<SessionsRail />);
    const trigger = screen.getByRole('button', { name: 'Project options' });
    fireEvent.click(trigger);
    const first = within(
      screen.getByRole('menu', { name: /Project options for KaruCode/ }),
    ).getAllByRole('menuitem')[0]!;
    await vi.waitFor(() => expect(document.activeElement).toBe(first));

    fireEvent.keyDown(first, { key: 'Escape' });
    // The menu owns the first Escape: the sheet stays open and focus comes back.
    expect(screen.queryByRole('menu')).toBeNull();
    expect(closeSheets).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(document.activeElement).toBe(trigger));
    closeSheets.mockRestore();
  });

  it('closes a session menu on Escape without closing the sheet', async () => {
    const closeSheets = vi.spyOn(actions, 'closeSheets').mockImplementation(() => undefined);
    render(<SessionsRail />);
    const trigger = screen.getByRole('button', { name: 'Actions for Fix login bug' });
    fireEvent.click(trigger);
    const first = within(
      screen.getByRole('menu', { name: 'Session actions for Fix login bug' }),
    ).getAllByRole('menuitem')[0]!;
    await vi.waitFor(() => expect(document.activeElement).toBe(first));

    fireEvent.keyDown(first, { key: 'Escape' });
    expect(screen.queryByRole('menu', { name: 'Session actions for Fix login bug' })).toBeNull();
    expect(closeSheets).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(trigger);
    closeSheets.mockRestore();
  });

  it('lets the sheet’s own controls handle Escape before it closes', () => {
    const closeSheets = vi.spyOn(actions, 'closeSheets').mockImplementation(() => undefined);
    render(<SessionsRail />);
    const search = screen.getByLabelText('Find a session') as HTMLInputElement;

    fireEvent.input(search, { target: { value: 'Fix' } });
    fireEvent.keyDown(search, { key: 'Escape' });
    // First press: clear the query. The sheet's capture-phase focus trap would
    // have closed everything here before the field ever saw the key.
    expect(search.value).toBe('');
    expect(closeSheets).not.toHaveBeenCalled();

    // Nothing left to clear: the second press is the sheet's.
    fireEvent.keyDown(search, { key: 'Escape' });
    expect(closeSheets).toHaveBeenCalledTimes(1);
    closeSheets.mockRestore();
  });

  it('lets an inline rename cancel with Escape without closing the sheet', () => {
    const closeSheets = vi.spyOn(actions, 'closeSheets').mockImplementation(() => undefined);
    render(<SessionsRail />);

    fireEvent.dblClick(screen.getByText('Fix login bug'));
    const input = screen.getByLabelText(/Rename session/) as HTMLInputElement;
    fireEvent.input(input, { target: { value: 'Discard this' } });
    fireEvent.keyDown(input, { key: 'Escape' });

    expect(screen.queryByLabelText(/Rename session/)).toBeNull();
    expect(closeSheets).not.toHaveBeenCalled();
    closeSheets.mockRestore();
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

describe('the collapsed rail’s fly-out (RailPeek)', () => {
  function seedPeek(overrides: { collapsed?: boolean } = {}): void {
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
        session_count: 1,
      },
      {
        id: 'p_docs',
        name: 'Docs Site',
        path: '/home/me/docs',
        created_at: NOW,
        updated_at: NOW,
        session_count: 1,
      },
    ];
    store.clock.value = NOW;
    store.narrowViewport.value = false;
    store.mobileSheet.value = null;
    store.railCollapsed.value = overrides.collapsed ?? true;
    store.settingsOpen.value = false;
    store.paletteOpen.value = false;
    store.themeStudioOpen.value = false;
    store.notificationCenterOpen.value = false;
    store.fileViewerPath.value = null;
    store.rewindTarget.value = null;
    railPeek.resetRailPeek();
  }

  beforeEach(() => {
    seedPeek();
    vi.spyOn(sessionFacts, 'loadFactsFor').mockResolvedValue(undefined);
  });

  it('replaces the rail with the hovered project’s sessions, not both', () => {
    railPeek.peekProjectId.value = 'p_docs';
    railPeek.peekSessions.value = { p_docs: [secondSummaryFixture] };
    const { container } = render(<SessionsRail />);

    const peek = container.querySelector('.rail-peek');
    expect(peek).not.toBeNull();
    expect(peek?.getAttribute('aria-label')).toBe('Sessions in Docs Site');
    expect(within(peek as HTMLElement).getByText('Add workspace tree')).not.toBeNull();
    // One list, one landmark: the full rail is not rendered behind the fly-out.
    expect(container.querySelectorAll('.rail')).toHaveLength(1);
    expect(within(peek as HTMLElement).queryByLabelText('Find a session')).toBeNull();
  });

  it('opens a session in another project by switching to it first', () => {
    const open = vi.spyOn(actions, 'openSessionInProject').mockResolvedValue(true);
    railPeek.peekProjectId.value = 'p_docs';
    railPeek.peekSessions.value = { p_docs: [secondSummaryFixture] };
    render(<SessionsRail />);

    fireEvent.click(screen.getByText('Add workspace tree'));
    expect(open).toHaveBeenCalledWith('p_docs', secondSummaryFixture.id);
    // The list has done its job; the chat is the destination.
    expect(railPeek.peekProjectId.value).toBeNull();
    open.mockRestore();
  });

  it('opens a session in the project already on screen without a project switch', () => {
    const select = vi.spyOn(actions, 'switchSession').mockImplementation(() => undefined);
    railPeek.peekProjectId.value = 'p_karucode';
    render(<SessionsRail />);

    fireEvent.click(screen.getByText('Fix login bug'));
    expect(select).toHaveBeenCalledWith(summaryFixture.id);
    expect(railPeek.peekProjectId.value).toBeNull();
    select.mockRestore();
  });

  it('offers to open the project when the peeked one is somewhere else', () => {
    const project = vi.spyOn(actions, 'switchProject').mockResolvedValue(undefined);
    railPeek.peekProjectId.value = 'p_docs';
    railPeek.peekSessions.value = { p_docs: [secondSummaryFixture] };
    render(<SessionsRail />);

    fireEvent.click(screen.getByRole('button', { name: 'Open Docs Site' }));
    expect(project).toHaveBeenCalledWith('p_docs');
    expect(railPeek.peekProjectId.value).toBeNull();
    project.mockRestore();
  });

  it('starts a draft from the peek when it is already the open project', () => {
    const draft = vi.spyOn(actions, 'startDraftSession').mockImplementation(() => {});
    railPeek.peekProjectId.value = 'p_karucode';
    render(<SessionsRail />);

    fireEvent.click(screen.getByRole('button', { name: /New session/ }));
    expect(draft).toHaveBeenCalledTimes(1);
    expect(railPeek.peekProjectId.value).toBeNull();
    draft.mockRestore();
  });

  it('switches to the peeked project before drafting in it', async () => {
    // A draft belongs to the *open* project. An empty preview of another project
    // offered "New session" that would silently start the conversation somewhere
    // else; the switch has to happen first.
    const order: string[] = [];
    const project = vi.spyOn(actions, 'switchProject').mockImplementation(async (id) => {
      order.push(`switch:${id}`);
      store.currentProjectId.value = id;
    });
    const draft = vi.spyOn(actions, 'startDraftSession').mockImplementation(() => {
      order.push('draft');
    });
    railPeek.peekProjectId.value = 'p_docs';
    railPeek.peekSessions.value = { p_docs: [] };
    render(<SessionsRail />);

    fireEvent.click(screen.getByRole('button', { name: 'New session' }));
    await vi.waitFor(() => expect(order).toEqual(['switch:p_docs', 'draft']));
    expect(railPeek.peekProjectId.value).toBeNull();
    project.mockRestore();
    draft.mockRestore();
  });

  it('aborts the draft when another project wins while the peeked switch is pending', async () => {
    let resolveSwitch!: () => void;
    const switching = new Promise<void>((resolve) => {
      resolveSwitch = resolve;
    });
    const project = vi.spyOn(actions, 'switchProject').mockImplementation(() => switching);
    const draft = vi.spyOn(actions, 'startDraftSession').mockImplementation(() => {});
    store.projects.value = [
      ...store.projects.value,
      {
        id: 'p_other',
        name: 'Other',
        path: '/home/me/other',
        created_at: NOW,
        updated_at: NOW,
        session_count: 0,
      },
    ];
    railPeek.peekProjectId.value = 'p_docs';
    railPeek.peekSessions.value = { p_docs: [] };
    render(<SessionsRail />);

    fireEvent.click(screen.getByRole('button', { name: 'New session' }));
    expect(project).toHaveBeenCalledWith('p_docs');
    expect(draft).not.toHaveBeenCalled();

    // A newer navigation finishes first. When the stale Docs switch resolves,
    // drafting now would start the conversation in Other, not the previewed one.
    store.currentProjectId.value = 'p_other';
    resolveSwitch();
    await switching;
    await Promise.resolve();

    expect(draft).not.toHaveBeenCalled();
    project.mockRestore();
    draft.mockRestore();
  });

  it('closes on Escape, which must not also stop the run', () => {
    railPeek.peekProjectId.value = 'p_karucode';
    render(<SessionsRail />);

    fireEvent.keyDown(window, { key: 'Escape' });
    expect(railPeek.peekProjectId.value).toBeNull();
  });

  it('says why a project’s sessions could not be read, and offers a retry', () => {
    const load = vi.spyOn(railPeek, 'loadPeekSessions').mockResolvedValue(undefined);
    railPeek.peekProjectId.value = 'p_docs';
    railPeek.peekError.value = { p_docs: 'the server said no' };
    render(<SessionsRail />);

    expect(screen.getByText('Could not read those sessions')).not.toBeNull();
    expect(screen.getByText('the server said no')).not.toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(load).toHaveBeenCalledWith('p_docs', true);
    load.mockRestore();
  });

  it('is not rendered while the rail is open, however the peek was left', () => {
    railPeek.peekProjectId.value = 'p_docs';
    store.railCollapsed.value = false;
    const { container } = render(<SessionsRail />);
    expect(container.querySelector('.rail-peek')).toBeNull();
    expect(screen.getByLabelText('Find a session')).not.toBeNull();
  });
});
