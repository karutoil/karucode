/**
 * The control plane's session region: the header, the facts and the subagents.
 *
 * The second half of this file is the §5A contract for this region: the session
 * view must be drawn from the workbench's own vocabulary (`.main-head`,
 * `.inspector-section`, `.metric`, `.agent-card`, `.icon-button`, `ExternalIcon`)
 * rather than a private `.cp-*` copy of it. Those are the things jsdom *can* check
 * without a stylesheet - the class a role is built from, the status that drives a
 * card's paint, whether a glyph is a character or an icon - and they are exactly
 * the things that regress silently when someone re-adds a local copy.
 *
 * The `vi.mock` blocks below are a per-file copy on purpose: `vi.mock` is
 * hoisted per test file, so a shared fixture module cannot register them.
 */
import { readFileSync } from 'node:fs';
import type { JSX } from 'preact';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import * as store from '../../state/store';
import * as actions from '../../state/actions';
import { resetControlPlane } from '../../state/controlPlane';
import { apiRequest } from '../../api/client';
import { getControlOverview, listAgents, listSessions } from '../../api/endpoints';
import { ControlPlane } from './ControlPlane';
import { ControlSession, type ControlSessionProps } from './ControlSession';
import {
  NOW,
  agent,
  chats,
  configWith,
  controlChat,
  overviewWith,
  sessions,
} from './controlPlaneFixtures';

vi.mock('../../api/client', () => ({
  apiRequest: vi.fn(),
  toErrorMessage: (error: unknown) => (error instanceof Error ? error.message : String(error)),
}));
vi.mock('../../api/endpoints', () => ({
  listSessions: vi.fn(),
  listAgents: vi.fn(),
  getControlOverview: vi.fn(),
}));
vi.mock('../../state/actions', () => ({
  openSession: vi.fn(async (id: string) => { store.currentSessionId.value = id; return true; }),
  switchProject: vi.fn(async (id: string) => { store.currentProjectId.value = id; }),
  fillComposer: vi.fn(),
  setSessionModelSelection: vi.fn(async () => true),
  clearSessionModelSelection: vi.fn(async () => true),
  openSettings: vi.fn(),
  openAgentDetail: vi.fn(async () => {}),
  closeAgentDetail: vi.fn(),
  stopRun: vi.fn(async () => {}),
  renameSessionById: vi.fn(async () => true),
  // The plane's delete path closes the open session through this action; the
  // real one clears the id, the transcript and the remembered pin.
  closeSession: vi.fn(() => { store.currentSessionId.value = null; }),
}));
// The real `ChatView` is the whole conversation; the region only composes it, so
// the stub renders the two props this region passes and nothing else. It renders
// `emptyContent` as *content* (not as a marker) so the first-run intro’s own
// markup can be asserted through it.
vi.mock('../ChatView', () => ({
  ChatView: (props: { emptyContent?: JSX.Element; mentionProjects?: { name: string }[] }) =>
    <div>Normal chat{props.emptyContent}{props.mentionProjects ? <div>Mentions: {props.mentionProjects.map((p) => p.name).join(', ')}</div> : null}</div>,
}));

/**
 * The region's own sheet, as text. Component tests run with `css: false`, so a
 * rule another file depends on has to be read from the source - the same approach
 * `styles/workflow-panels.test.ts` takes. The path is relative to the runner's
 * root (`web/`), as in `theme/tokens.test.ts`; `?raw` cannot be used because
 * `css: false` stubs every `.css` import, `?raw` included.
 */
const sheet = readFileSync('src/styles/control-session.css', 'utf8');

const projectName = (id: string | null | undefined): string =>
  id ? store.projects.value.find((project) => project.id === id)?.name ?? id : 'No project';

/** Render one open session directly: no rail, no shell, just this region. */
function openSession(overrides: Partial<ControlSessionProps> = {}) {
  return render(<ControlSession
    selected="s1"
    chosen={sessions[0]!}
    control={false}
    opening={false}
    ready
    detailAgents={[]}
    claimedTotal={null}
    projects={store.projects.value}
    projectName={projectName}
    onBack={() => {}}
    onOpenInProject={() => {}}
    onNavigate={() => {}}
    {...overrides}
  />);
}

beforeEach(() => {
  vi.clearAllMocks();
  // The snapshot lives in module signals; without this a previous test's
  // overview is still on screen when the next one mounts.
  resetControlPlane();
  store.projects.value = [
    { id: 'p1', name: 'One', path: '/p1', created_at: 0, updated_at: 0, session_count: 1 },
    { id: 'p2', name: 'Two', path: '/p2', created_at: 0, updated_at: 0, session_count: 1 },
  ];
  store.currentProjectId.value = 'p1';
  store.currentSessionId.value = null;
  store.chat.value = { sessionId: null, items: [], queuedSteers: [], queuedFollowUps: [] } as never;
  store.config.value = configWith([
    { id: 'default', name: 'Default', models: [{ id: 'gpt-4o-mini', name: 'GPT-4o mini' }] },
  ]);
  store.selectedAgentId.value = null;
  store.agentHistory.value = null;
  vi.mocked(actions.openSession).mockImplementation(async (id: string) => {
    store.currentSessionId.value = id;
    return true;
  });
  vi.mocked(actions.switchProject).mockImplementation(async (id: string) => {
    store.currentProjectId.value = id;
  });
  // `/api/sessions` lists control chats too (it is the same table); the rail has
  // to keep them out of the project group by their flag.
  vi.mocked(listSessions).mockResolvedValue([...sessions, ...chats] as never);
  // The open conversation reads its own session's subagents; the default is "a
  // session that delegated nothing", which each subagent test overrides.
  vi.mocked(listAgents).mockResolvedValue([]);
  vi.mocked(getControlOverview).mockResolvedValue(overviewWith());
  vi.mocked(apiRequest).mockImplementation((async (path: string, options?: { method?: string; body?: { title?: string } }) => {
    if (path === '/api/control/sessions') {
      return options?.method === 'POST'
        ? { session_id: 'c-new', project_id: null, workspace: '/data/control-workspaces/c-new', created: true }
        : { sessions: chats };
    }
    if (options?.method === 'PATCH') return { id: path.split('/').pop(), title: options.body?.title };
    if (options?.method === 'DELETE') return { deleted: true };
    throw new Error(`unexpected ${path}`);
  }) as never);
});

/* ---------------------------------------------------------------- subagents */

describe('the open chat’s subagents', () => {
  it('lists the session’s own agents, whatever their status', async () => {
    // The rail badge comes from the snapshot (live only); the inspector reads the
    // session itself, so a finished child is still listed - which is the whole
    // point of "what did this chat delegate".
    vi.mocked(listAgents).mockResolvedValue([
      agent('researcher', { note: 'reading the control plane', iterations: 4 }),
      agent('reviewer', { status: 'completed', finished_at: NOW - 10_000, note: '' }),
    ]);
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^Control chat/ }));
    await screen.findByText('Normal chat');

    const section = await screen.findByLabelText('Subagents of this chat');
    await waitFor(() => expect(section.querySelectorAll('.agent-card')).toHaveLength(2));
    expect(vi.mocked(listAgents)).toHaveBeenCalledWith({ session: 'c1' });
    const rows = section.querySelectorAll('.agent-card');
    expect(rows[0]?.textContent).toContain('researcher');
    expect(rows[0]?.textContent).toContain('running');
    expect(rows[0]?.textContent).toContain('reading the control plane');
    expect(rows[0]?.textContent).toContain('4 iterations');
    expect(rows[1]?.textContent).toContain('reviewer');
    expect(rows[1]?.textContent).toContain('done');

    // "Details" reads the agent's own transcript through the existing store.
    fireEvent.click(within(section as HTMLElement).getByRole('button', { name: 'Open details for researcher' }));
    expect(actions.openAgentDetail).toHaveBeenCalledWith('researcher');
  });

  it('falls back to the snapshot’s live agents until its own read lands', async () => {
    // A read that never answers must not blank the list: the snapshot already
    // knows who is working.
    vi.mocked(listAgents).mockImplementation(() => new Promise(() => {}));
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      live_agents: [agent('researcher')],
    }));
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^Control chat/ }));
    await screen.findByText('Normal chat');
    const section = await screen.findByLabelText('Subagents of this chat');
    expect(section.textContent).toContain('researcher');
    expect(section.textContent).toContain('1 live');
  });

  it('says so when the chat has delegated nothing', async () => {
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      live_agents: [],
      control: [controlChat({ id: 'c1', title: 'Control chat' })],
    }));
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^Control chat/ }));
    await screen.findByText('Normal chat');
    const section = await screen.findByLabelText('Subagents of this chat');
    expect(within(section as HTMLElement).getByText(/No subagents yet/)).toBeTruthy();
  });

  it('never claims a chat with finished children delegated nothing', async () => {
    // The session's read is what answers this, so two completed children are two
    // rows - not "no subagents yet" and not "none loaded yet".
    vi.mocked(listAgents).mockResolvedValue([
      agent('first', { status: 'completed', finished_at: NOW - 20_000 }),
      agent('second', { status: 'cancelled', finished_at: NOW - 10_000 }),
    ]);
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      live_agents: [],
      control: [controlChat({ id: 'c1', title: 'Control chat', agents: { live: 0, total: 2 } })],
    }));
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^Control chat/ }));
    await screen.findByText('Normal chat');
    const section = await screen.findByLabelText('Subagents of this chat');
    await waitFor(() => expect(section.querySelectorAll('.agent-card')).toHaveLength(2));
    expect(section.textContent).toContain('done');
    expect(section.textContent).toContain('cancelled');
    expect(within(section as HTMLElement).queryByText(/No subagents yet/)).toBeNull();
    expect(within(section as HTMLElement).queryByText(/none of them loaded yet/)).toBeNull();
  });

  it('drops the previous chat’s subagents when another chat opens', async () => {
    // The rows are read per session, so switching chats must clear them first:
    // showing chat A's children under chat B's heading (until B's read lands) is
    // the same class of lie as a stale title.
    vi.mocked(listAgents).mockImplementation((async (options?: { session?: string }) => {
      if (options?.session === 'c1') return [agent('first', { session_id: 'c1' })];
      return new Promise(() => {}); // the second chat's read never answers
    }) as never);
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      live_agents: [],
      control: [
        controlChat({ id: 'c1', title: 'First chat' }),
        controlChat({ id: 'c2', title: 'Second chat' }),
      ],
    }));
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^First chat/ }));
    await screen.findByText('Normal chat');
    const section = await screen.findByLabelText('Subagents of this chat');
    await waitFor(() => expect(section.textContent).toContain('first'));

    fireEvent.click(within(group).getByRole('button', { name: /^Second chat/ }));
    // Re-query: the inspector is re-rendered for the new session, so the element
    // captured above may be detached by now.
    await waitFor(() =>
      expect(screen.getByLabelText('Subagents of this chat').textContent).not.toContain('first'),
    );
    expect(within(screen.getByLabelText('Subagents of this chat') as HTMLElement).getByText(/No subagents yet/)).toBeTruthy();
  });
});

/* ---------------------------------------------------------------- the header */

describe('the open session’s header', () => {
  it('states the session title exactly once', () => {
    // The defect this region had: with a chat open, `.cp-session-head` and the
    // embedded `ChatView`’s own header both named the session. One owner per
    // fact - and the owner is this header, because it is the only one that can
    // also offer Back and "Open in project".
    const chosen = { ...sessions[0]!, title: 'Release audit' };
    const { container } = openSession({ chosen });
    expect(within(container as HTMLElement).queryAllByText('Release audit')).toHaveLength(1);
    // One heading, and one header element, for the whole region.
    expect(container.querySelectorAll('header.main-head')).toHaveLength(1);
    expect(container.querySelectorAll('h1, h2')).toHaveLength(1);
    expect(container.querySelector('header.cp-session-head')).toBeTruthy();
    // ...and the embedded conversation’s header stays suppressed. jsdom applies
    // no stylesheets, so the rule that does it is read from the sheet itself.
    expect(sheet).toMatch(/\.cp-chat \.main-head \{ display: none; \}/);
    // The load-bearing half of the pair: the component emits the wrapper the
    // sheet's rule keys on. Rename `.cp-chat` in the component and the rule
    // matches nothing while jsdom stays green and the app shows the title twice.
    // (The other half - that nothing visible renders a second header - is the
    // browser harness's job: `verify_control_parity.py` counts visible headers.)
    expect(container.querySelector('.cp-chat')).toBeTruthy();

    // The same for a control chat, whose first-run intro is a *different*
    // heading - not a second statement of the session's title.
    const chat = openSession({ chosen: controlChat({ id: 'c1', title: 'Release audit' }), control: true });
    expect(within(chat.container as HTMLElement).queryAllByText('Release audit')).toHaveLength(1);
    expect(within(chat.container as HTMLElement).queryAllByText('What should we build?')).toHaveLength(1);
  });

  it('heads the pane with the workbench’s header, breadcrumb and title classes', () => {
    openSession({ chosen: controlChat({ id: 'c1', title: 'Release audit' }), control: true });
    const head = screen.getByLabelText('Open session').querySelector('header');
    expect(head?.classList.contains('main-head')).toBe(true);
    const crumb = head?.querySelector('.breadcrumb');
    expect(crumb?.classList.contains('cp-session-id')).toBe(true);
    // Where this conversation lives, then its title - the workbench’s order.
    expect(crumb?.querySelector('strong')?.textContent).toBe('Control plane · across projects');
    expect(crumb?.querySelector('h2')?.classList.contains('main-title')).toBe(true);
    expect(crumb?.querySelector('h2')?.textContent).toBe('Release audit');
  });

  it('makes the way back an icon button, not an arrow character', () => {
    openSession();
    const back = screen.getByRole('button', { name: 'Back to overview' });
    expect(back.classList.contains('icon-button')).toBe(true);
    expect(back.querySelector('svg')).toBeTruthy();
    expect(back.textContent?.trim()).toBe('');
  });

  it('says "open in project" with the external icon, not a glyph', () => {
    openSession();
    const open = screen.getByRole('button', { name: 'Open in project' });
    expect(open.classList.contains('button')).toBe(true);
    expect(open.classList.contains('button-small')).toBe(true);
    expect(open.querySelector('svg')).toBeTruthy();
    expect(open.textContent).toContain('Open in project');
    expect(open.textContent).not.toContain('↗');
  });

  it('offers no "open in project" for a control chat', () => {
    openSession({ chosen: controlChat({ id: 'c1' }), control: true });
    expect(screen.queryByRole('button', { name: 'Open in project' })).toBeNull();
  });
});

/* ----------------------------------------------------------------- the facts */

describe('the session’s own facts', () => {
  it('is the inspector’s section, with metric tiles for the numbers', () => {
    openSession();
    const facts = screen.getByLabelText('Session details');
    expect(facts.classList.contains('inspector-section')).toBe(true);
    expect(facts.querySelector('.inspector-kicker')?.textContent).toBe('Session');
    const tiles = facts.querySelectorAll('.metric');
    expect(tiles).toHaveLength(2);
    expect(tiles[0]?.querySelector('.metric-num')?.textContent).toBe('1');
    expect(tiles[0]?.querySelector('.metric-label')?.textContent).toBe('Messages');
    expect(tiles[1]?.querySelector('.metric-label')?.textContent).toBe('Last activity');
  });

  it('lists the path and the session id, with the id copyable', () => {
    openSession();
    const facts = screen.getByLabelText('Session details');
    const labels = [...facts.querySelectorAll('dt')].map((dt) => dt.textContent);
    // No "State" row: the header's `.cp-state` word owns that fact.
    expect(labels).toEqual(['Path', 'Session id']);
    expect(facts.textContent).toContain('/p1');
    const idRow = facts.querySelector('.cp-facts-id');
    expect(idRow?.textContent).toContain('s1');
    const copy = within(facts as HTMLElement).getByRole('button', { name: 'Copy' });
    expect(copy.classList.contains('link-button')).toBe(true);
  });

  it('says "control plane" for a chat and keeps its id, without a path', () => {
    openSession({ chosen: controlChat({ id: 'c1', title: 'Release audit' }), control: true });
    const facts = screen.getByLabelText('Session details');
    expect([...facts.querySelectorAll('dt')].map((dt) => dt.textContent)).toEqual(['Session id']);
    expect(facts.textContent).toContain('c1');
  });

  it('offers to stop a run only while one is in flight', () => {
    const stopping = openSession();
    fireEvent.click(screen.getByRole('button', { name: 'Stop this run' }));
    expect(actions.stopRun).toHaveBeenCalledWith();
    stopping.unmount();
    openSession({ chosen: sessions[1]! });
    expect(screen.queryByRole('button', { name: 'Stop this run' })).toBeNull();
  });
});

/* -------------------------------------------------------------- agent cards */

describe('the subagent rows', () => {
  it('are the workbench’s agent cards, each carrying its status', () => {
    openSession({
      detailAgents: [
        agent('researcher', { progress: 0.4 }),
        agent('reviewer', { status: 'completed', finished_at: NOW, note: '' }),
      ],
    });
    const rows = [...screen.getByLabelText('Subagents of this chat').querySelectorAll('.agent-card')];
    expect(rows.map((row) => row.getAttribute('data-status'))).toEqual(['running', 'completed']);
    // The status drives the paint (`.agent-card[data-status='running']`,
    // `.agent-live`), so the card’s own vocabulary is what has to be rendered.
    expect(rows[0]?.querySelector('.agent-head .agent-name')?.textContent).toBe('researcher');
    expect(rows[0]?.querySelector('.agent-role')?.textContent).toBe('researcher');
    expect(rows[0]?.querySelector('.agent-live')?.textContent).toBe('running');
    expect(rows[1]?.querySelector('.agent-live')?.textContent).toBe('done');
    expect(rows[0]?.querySelector('.agent-progress')?.getAttribute('aria-valuenow')).toBe('40');
    expect(rows[1]?.querySelector('.agent-progress')).toBeNull();
  });

  it('opens its transcript inline and says when there is nothing to read', () => {
    openSession({ detailAgents: [agent('researcher')] });
    const section = screen.getByLabelText('Subagents of this chat');
    const button = within(section as HTMLElement).getByRole('button', { name: 'Open details for researcher' });
    expect(button.getAttribute('aria-expanded')).toBe('false');
    expect(button.textContent).toContain('Details');
    fireEvent.click(button);
    expect(actions.openAgentDetail).toHaveBeenCalledWith('researcher');
  });

  it('shows the read transcript, and closes it again', () => {
    store.selectedAgentId.value = 'researcher';
    store.agentHistory.value = {
      id: 'researcher',
      messages: [{ id: 'm1', role: 'assistant', content: 'Found it.', created_at: NOW }],
      truncated: false,
    } as never;
    openSession({ detailAgents: [agent('researcher')] });
    const section = screen.getByLabelText('Subagents of this chat');
    expect(section.textContent).toContain('Found it.');
    expect(section.textContent).toContain('Its whole transcript.');
    const close = within(section as HTMLElement).getByRole('button', { name: 'Close details for researcher' });
    expect(close.getAttribute('aria-expanded')).toBe('true');
    fireEvent.click(close);
    expect(actions.closeAgentDetail).toHaveBeenCalled();
  });
});

/* --------------------------------------------------------------- the intro */

describe('the empty control chat', () => {
  it('is the workbench’s empty state, with a button per project', () => {
    openSession({ chosen: controlChat({ id: 'c1', title: '' }), control: true });
    // The intro is rendered into `ChatView`’s `emptyContent`, so it arrives here
    // through the stub rather than from this region’s own output.
    const intro = screen.getByText('What should we build?').closest('.empty-state');
    expect(intro).toBeTruthy();
    expect(intro?.querySelector('.empty-symbol')).toBeTruthy();
    expect(intro?.querySelector('.empty-title')?.textContent).toBe('What should we build?');
    expect(intro?.querySelector('.empty-copy')?.textContent).toContain('Mention a project');
    const buttons = [...(intro?.querySelectorAll('.cp-intro-projects button') ?? [])];
    expect(buttons.map((button) => button.textContent)).toEqual(['One', 'Two']);
    expect(buttons[0]?.classList.contains('button')).toBe(true);
    fireEvent.click(buttons[1]!);
    expect(actions.fillComposer).toHaveBeenCalledWith('Start work in project "Two" (project_id: p2).\n\n');
  });
});
