/**
 * The control plane's overview region: the launch box and the workspace
 * snapshot. Split out of `ControlPlane.test.tsx` so an overview change never
 * has to touch the rail's or the session's tests.
 *
 * The `vi.mock` blocks below are a per-file copy on purpose: `vi.mock` is
 * hoisted per test file, so a shared fixture module cannot register them.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import * as store from '../../state/store';
import * as actions from '../../state/actions';
import { resetControlPlane } from '../../state/controlPlane';
import { apiRequest } from '../../api/client';
import { getControlOverview, listAgents, listSessions } from '../../api/endpoints';
import { ControlPlane } from './ControlPlane';
import { REASONING_OPTIONS } from '../ModelSwitcher';
import {
  agent,
  chats,
  configWith,
  controlChat,
  overviewWith,
  projectSession,
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
vi.mock('../ChatView', () => ({
  ChatView: (props: { emptyContent?: unknown; mentionProjects?: { name: string }[] }) =>
    <div>Normal chat{props.emptyContent ? <div>Control intro</div> : null}{props.mentionProjects ? <div>Mentions: {props.mentionProjects.map((p) => p.name).join(', ')}</div> : null}</div>,
}));

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

/* ------------------------------------------------------------------ launch */

describe('the launch box', () => {
  it('carries a launch draft into the control chat composer', async () => {
    render(<ControlPlane />);
    const input = await screen.findByLabelText('Start something with control chat');
    fireEvent.input(input, { target: { value: 'Add a health endpoint to the API' } });
    fireEvent.click(screen.getByRole('button', { name: /Continue/ }));
    await waitFor(() => expect(actions.fillComposer).toHaveBeenCalledWith(
      expect.stringContaining('Add a health endpoint to the API'),
    ));
    expect(vi.mocked(actions.fillComposer).mock.calls[0]?.[0]).toContain('(project_id: p1)');
  });

  it('pins the chosen model before handing over the draft', async () => {
    store.config.value = configWith([
      { id: 'default', name: 'Default', models: [{ id: 'gpt-4o-mini', name: 'GPT-4o mini', reasoning: true }] },
      { id: 'fast', name: 'Fast', models: [{ id: 'gpt-5-mini', name: 'GPT-5 mini' }] },
    ]);
    const order: string[] = [];
    vi.mocked(actions.setSessionModelSelection).mockImplementation(async () => { order.push('pin'); return true; });
    vi.mocked(actions.fillComposer).mockImplementation(() => { order.push('fill'); });

    render(<ControlPlane />);
    const model = await screen.findByLabelText('Model for the new chat') as HTMLSelectElement;
    fireEvent.change(model, { target: { value: 'fast::gpt-5-mini' } });
    fireEvent.change(screen.getByLabelText('Reasoning for the new chat'), { target: { value: 'high' } });
    fireEvent.input(screen.getByLabelText('Start something with control chat'), { target: { value: 'Ship it' } });
    fireEvent.click(screen.getByRole('button', { name: /Continue/ }));

    await waitFor(() => expect(order).toEqual(['pin', 'fill']));
    // `default` effort is sent as `null`: the endpoint decides, which is what the
    // composer's own selector does when it shows "Default".
    expect(actions.setSessionModelSelection).toHaveBeenCalledWith('fast', 'gpt-5-mini', 'high');
    expect(actions.fillComposer).toHaveBeenCalledWith(expect.stringContaining('Ship it'));
  });

  it('does not offer reasoning for a model that cannot reason', async () => {
    store.config.value = configWith([
      { id: 'plain', name: 'Plain', models: [{ id: 'no-reasoning', name: 'No reasoning', reasoning: false }] },
    ]);
    render(<ControlPlane />);
    const model = await screen.findByLabelText('Model for the new chat');
    expect(model).toBeTruthy();
    // `reasoning: false` is a positive "cannot reason" — the same rule the
    // composer's ReasoningSelector follows.
    expect(screen.queryByLabelText('Reasoning for the new chat')).toBeNull();
  });

  it('retains the launch prompt when opening the control chat fails', async () => {
    let finish: ((ok: boolean) => void) | undefined;
    vi.mocked(actions.openSession).mockImplementation(
      () => new Promise<boolean>((resolve) => { finish = resolve; }),
    );
    render(<ControlPlane />);
    const input = await screen.findByLabelText('Start something with control chat') as HTMLInputElement;
    fireEvent.input(input, { target: { value: 'Retry this prompt' } });
    fireEvent.click(screen.getByRole('button', { name: /Continue/ }));
    await waitFor(() => expect(actions.openSession).toHaveBeenCalled());
    await act(async () => {
      finish?.(false);
      await Promise.resolve();
    });

    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('Could not open session.'));
    fireEvent.click(screen.getByRole('button', { name: 'Back to overview' }));
    expect((await screen.findByLabelText('Start something with control chat') as HTMLInputElement).value).toBe(
      'Retry this prompt',
    );
    expect(actions.fillComposer).not.toHaveBeenCalled();
  });

  it('does not overwrite edits made while a successful launch is pending', async () => {
    let finish: ((ok: boolean) => void) | undefined;
    vi.mocked(actions.openSession).mockImplementation(
      () => new Promise<boolean>((resolve) => { finish = resolve; }),
    );
    render(<ControlPlane />);
    const input = await screen.findByLabelText('Start something with control chat') as HTMLInputElement;
    fireEvent.input(input, { target: { value: 'submitted prompt' } });
    fireEvent.click(screen.getByRole('button', { name: /Continue/ }));
    await waitFor(() => expect(actions.openSession).toHaveBeenCalled());
    fireEvent.input(input, { target: { value: 'edited while pending' } });
    const opened = vi.mocked(actions.openSession).mock.calls[0]?.[0];
    await act(async () => {
      store.currentSessionId.value = opened ?? null;
      finish?.(true);
      await Promise.resolve();
    });

    await waitFor(() => expect(actions.fillComposer).toHaveBeenCalledWith(
      expect.stringContaining('submitted prompt'),
    ));
    // The input is unmounted as soon as c1 opens, so the prompt was accepted.
    expect(screen.queryByLabelText('Start something with control chat')).toBeNull();
    // Re-open the overview to inspect the preserved value without remounting state.
    fireEvent.click(screen.getByRole('button', { name: 'Back to overview' }));
    expect((await screen.findByLabelText('Start something with control chat') as HTMLInputElement).value).toBe(
      'edited while pending',
    );
  });
});

/* ---------------------------------------------------------------- overview */

describe('the overview snapshot', () => {
  it('drives every counter from the one read', async () => {
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      sessions: { total: 9, running: 4, failed: 2, control: 3 },
      agents: { live: 5, paused: 1, failed: 0, total: 8 },
      projects: [
        { id: 'p1', name: 'One', path: '/p1', sessions: 4, running: 3 },
        { id: 'p2', name: 'Two', path: '/p2', sessions: 5, running: 1 },
        { id: 'p3', name: 'Three', path: '/p3', sessions: 0, running: 0 },
      ],
    }));
    render(<ControlPlane />);
    const summary = await screen.findByLabelText('Workspace summary');
    const counters = [...summary.querySelectorAll('.metric')].map((cell) => [
      cell.querySelector('.metric-num')?.textContent,
      cell.querySelector('.metric-label')?.textContent,
    ]);
    expect(counters).toEqual([
      ['4', 'running'],
      ['2', 'chats'],
      ['9', 'sessions'],
      ['5', 'agents'],
      ['3', 'projects'],
    ]);
    // The per-project counts are the snapshot's, not a recount of the rail.
    const projects = screen.getByLabelText('Projects');
    expect(within(projects).getByText('4 sessions · 3 running')).toBeTruthy();
    expect(within(projects).getByText('0 sessions')).toBeTruthy();
  });

  it('lists the snapshot’s attention, running, recent and live agents', async () => {
    const failed = projectSession({ id: 's_failed', title: 'Broken build', project_id: 'p2', last_run_status: 'failed' });
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      attention: [failed],
      live_agents: [agent('reviewer', { note: 'reading the diff' })],
    }));
    render(<ControlPlane />);
    expect(await screen.findByText('Needs attention')).toBeTruthy();
    expect(screen.getByText('Broken build')).toBeTruthy();
    expect(screen.getByText('reading the diff')).toBeTruthy();
    // The live agent is the workbench's own card, and its one action is the way
    // back to the session it belongs to (`agent-card-open`, the 24px target).
    expect(screen.getByRole('button', { name: 'Open Control chat' })).toBeTruthy();
  });

  it('calls out a run parked on an approval, rather than calling it running', async () => {
    const stalled = projectSession({ id: 's_wait', title: 'Awaiting approval', project_id: 'p2', running: true });
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      approvals: { pending: 1 },
      waiting: [stalled],
      control: [controlChat({ id: 'c1', title: 'Control chat', running: true, pending_approvals: 2 })],
    }));
    render(<ControlPlane />);
    // Above the dashboard: this is the one thing that cannot progress alone.
    const banner = await screen.findByText(/waiting on you — open the session/);
    expect(banner.textContent).toContain('1');

    const waiting = screen.getByLabelText('Waiting on you');
    expect(within(waiting).getByText('Awaiting approval')).toBeTruthy();
    // The row is the workbench's `.session-row` shape (a `.status-dot` plus a
    // meta line), so the words live inside that line rather than a badge of their
    // own - and a run parked on an approval still *says* it needs a person.
    expect(within(waiting).getByText(/Needs you/)).toBeTruthy();
    expect(waiting.querySelector('.session-row.needs .status-dot.status-warn')).toBeTruthy();

    // The chat's own row says the same thing instead of the misleading "Running".
    const row = within(screen.getByLabelText('Control chats')).getByRole('button', { name: /^Control chat/ });
    expect(within(row).getByText('Needs you')).toBeTruthy();
  });

  it('marks a failed read stale without erasing the last snapshot', async () => {
    render(<ControlPlane />);
    const summary = await screen.findByLabelText('Workspace summary');
    expect(summary.querySelector('.metric-num')?.textContent).toBe('1');

    vi.mocked(getControlOverview).mockRejectedValue(new Error('HTTP 503'));
    fireEvent.click(screen.getByRole('button', { name: 'Refresh sessions' }));
    await waitFor(() => expect(screen.getByText(/stale · HTTP 503/)).toBeTruthy());
    // The numbers are still the last good read: "nothing is running" is a
    // different answer, and a wrong one.
    expect(screen.getByLabelText('Workspace summary').querySelector('.metric-num')?.textContent).toBe('1');
  });
});

/* ------------------------------------------------------- shared vocabulary
 *
 * §5A of `docs/internal/DESIGN-SYSTEM.md`: an element that plays a role the
 * workbench already has wears the workbench's class. jsdom cannot measure a
 * contrast ratio or a layout, but it can prove *adoption* - that the shared class
 * is the one the overview actually renders - and that was previously only checked
 * by a browser script.
 */

describe('the overview speaks the product’s vocabulary', () => {
  it('draws the hero counters with the workbench’s metric recipe', async () => {
    render(<ControlPlane />);
    const summary = await screen.findByLabelText('Workspace summary');
    const cells = [...summary.querySelectorAll('.metric')];
    expect(cells).toHaveLength(5);
    for (const cell of cells) {
      expect(cell.querySelector('.metric-num')?.textContent).toMatch(/^\d+$/);
      expect(cell.querySelector('.metric-label')?.textContent?.trim()).not.toBe('');
    }
    // …and no private number-plus-label markup survives beside it: every child of
    // the summary is a `.metric`, so the two cannot drift apart again.
    expect([...summary.children].every((child) => child.classList.contains('metric'))).toBe(true);
  });

  it('offers exactly the reasoning efforts the composer offers', async () => {
    render(<ControlPlane />);
    const reasoning = await screen.findByLabelText('Reasoning for the new chat') as HTMLSelectElement;
    // The launch box imports `REASONING_OPTIONS` rather than keeping its own
    // table, so a new effort cannot reach the composer and miss the launch box
    // (or the other way round). This asserts the source, not just the values.
    expect([...reasoning.options].map((option) => [option.value, option.textContent])).toEqual(
      REASONING_OPTIONS.map((option) => [option.value, option.label]),
    );
  });

  it('draws icons, never a text glyph', async () => {
    const { container } = render(<ControlPlane />);
    await screen.findByLabelText('Workspace summary');
    const overview = container.querySelector('.cp-overview');
    expect(overview).toBeTruthy();
    const text = overview?.textContent ?? '';
    expect(text.length).toBeGreaterThan(0);
    // The characters §5A retires: they have no consistent metrics, weight or
    // baseline, and the icon set already publishes every one of these shapes.
    for (const glyph of ['⟳', '⌕', '＋', '→', '✳', '↗', '←', '✕']) {
      expect(text.includes(glyph), `${glyph} is still drawn as text`).toBe(false);
    }
    // The two marks the launch box and the cards do use are real icons.
    expect(overview?.querySelectorAll('svg').length).toBeGreaterThan(0);
  });

  it('builds its controls, kickers, counts and avatars from the shared classes', async () => {
    const { container } = render(<ControlPlane />);
    await screen.findByLabelText('Workspace summary');
    const overview = container.querySelector('.cp-overview') as HTMLElement;

    // The launch box's submit is the product's primary button, and the launch
    // field label is the product's kicker.
    expect(screen.getByRole('button', { name: /Open chat|Continue/ }).className).toContain('button-primary');
    expect(overview.querySelector('.cp-launch-label.eyebrow')?.textContent).toBe('Start something with control chat');
    expect(overview.querySelectorAll('.cp-launch input.input').length).toBe(1);
    expect(overview.querySelectorAll('.cp-launch select.input').length).toBe(3);

    // Section heads are a kicker plus the shared count badge.
    expect(overview.querySelectorAll('.cp-section-head > .eyebrow').length).toBeGreaterThan(0);
    expect(overview.querySelectorAll('.cp-section-head > .badge').length).toBeGreaterThan(0);

    // A project mark is the rail's avatar, and a card is the product's card.
    expect(overview.querySelectorAll('.rail-avatar').length).toBeGreaterThan(0);
    expect(overview.querySelectorAll('.cp-card.agent-card, .cp-project.agent-card').length).toBeGreaterThan(0);

    // And none of the private re-implementations §5A retires is rendered here.
    const retired = ['.cp-glyph', '.cp-primary', '.cp-secondary', '.cp-link', '.cp-eyebrow', '.cp-agent-dot'];
    for (const selector of retired) {
      expect(overview.querySelectorAll(selector).length, `${selector} is still rendered`).toBe(0);
    }
  });
});
