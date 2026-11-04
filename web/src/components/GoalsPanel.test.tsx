import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '../api/client';
import type { PluginPanelTab } from '../state/store';
import {
  findPluginPanelRenderer,
  registerPluginPanelRenderer,
  resetPluginPanelRenderers,
} from './pluginPanels';
import {
  GOALS_PANEL_ID,
  GoalsPanel,
  goalsPanelRenderer,
  optionText,
  parseGoalsPayload,
} from './GoalsPanel';

/**
 * The Goals panel renderer, exercised the way the shell uses it: through the
 * registry's `matches`/`render` contract and through `PluginPanelView`.
 *
 * The fixtures mirror what `panel()` in the plugin's `src/render.rs` produces.
 * That payload is the only thing tying the two halves together, so it is pinned
 * on both sides: a field renamed in Rust breaks a test here.
 */

const sendMessage = vi.fn(async (_session: string, _text: string) => ({
  run_id: 'r_next',
  session_id: 's_1',
}));
const sendQueuedMessage = vi.fn(
  async (_session: string, _mode: string, _message: { content: string }) => ({
    queued: true,
    follow_up_id: 'q_1',
  }),
);

vi.mock('../api/endpoints', () => ({
  sendMessage: (session: string, text: string) => sendMessage(session, text),
  sendQueuedMessage: (session: string, mode: string, message: { content: string }) =>
    sendQueuedMessage(session, mode, message),
}));

vi.mock('../state/actions', () => ({
  refreshPluginPanel: vi.fn(async () => undefined),
}));

function criterion(overrides: Record<string, unknown> = {}) {
  return {
    id: 'c1',
    text: 'the build is green',
    met: false,
    evidence: null,
    met_at: null,
    check: null,
    ...overrides,
  };
}

function option(overrides: Record<string, unknown> = {}) {
  return { key: 'max_continuations', label: 'Continuation cap', kind: 'number', value: 0, unit: 'resumes', zero_means: 'no cap', ...overrides };
}

function action(overrides: Record<string, unknown> = {}) {
  return {
    id: 'pause',
    label: 'Pause',
    kind: 'secondary',
    enabled: true,
    hint: 'Stops resuming; the goal is kept.',
    instruction: 'Call plugin__kc-goals__goal_pause for g1 with the reason you are stopping.',
    ...overrides,
  };
}

function goal(overrides: Record<string, unknown> = {}) {
  return {
    goal: 'g1',
    session_id: 's_1',
    is_current_session: true,
    title: 'Ship the widget',
    statement: 'Ship the widget end to end',
    status: 'active',
    source: 'user',
    met: 1,
    criteria: 2,
    score: '1/2',
    progress_pct: 50,
    updated: '2 min ago',
    continuations: 3,
    stalled_resumes: 0,
    pause_reason: null,
    engine_note: null,
    engine: 'engine: will resume this goal when the next run ends',
    budget_text: '3 continuations, no cap; no deadline; 0/5 stalled resumes',
    next: 'do the work for `c2`',
    checks: 1,
    checks_pending: 1,
    criteria_detail: [
      criterion({ id: 'c1', text: 'builds', met: true, evidence: 'cargo build --release' }),
      criterion({
        id: 'c2',
        text: 'tested',
        check: { command: 'cargo test -q', ok: null, checked_at: null },
      }),
    ],
    options: [option()],
    actions: [action()],
    journal: [{ ago: '1 min ago', note: 'wrote the renderer', next: 'test it' }],
    ...overrides,
  };
}

function payload(overrides: Record<string, unknown> = {}) {
  return {
    schema: 'kc-goals/panel/2',
    session_id: 's_1',
    workspace: '/w',
    open: 1,
    total: 1,
    goals: [goal()],
    log: [{ at: 'just now', session: 's_1', run: 'r1', kind: 'continue', outcome: 'posted (send)' }],
    engine: {
      enabled: true,
      auto_continue: true,
      prompt: true,
      verify_commands: true,
      options: [option(), option({ key: 'auto_continue', label: 'Resume stopped runs', kind: 'switch', value: true })],
      actions_note: 'A panel cannot call tools: each action sends its instruction to the session as a user turn.',
    },
    error: null,
    ...overrides,
  };
}

function panelTab(): PluginPanelTab {
  return {
    id: 'goals',
    hosted_id: GOALS_PANEL_ID,
    title: 'Goals',
    icon: 'check',
    source: 'static',
    refresh_ms: 5000,
    plugin: 'kc-goals',
    version: '0.1.0',
  };
}

afterEach(() => {
  cleanup();
  resetPluginPanelRenderers();
  sendMessage.mockClear();
  sendQueuedMessage.mockClear();
});

describe('parseGoalsPayload', () => {
  it('accepts the payload the plugin produces', () => {
    const parsed = parseGoalsPayload(payload());
    expect(parsed).toBeDefined();
    expect(parsed?.goals).toHaveLength(1);
    expect(parsed?.goals[0]?.title).toBe('Ship the widget');
  });

  it('declines a payload from another panel or an older one', () => {
    expect(parseGoalsPayload(null)).toBeUndefined();
    expect(parseGoalsPayload({ goals: [] })).toBeUndefined();
    expect(parseGoalsPayload({ schema: 'kc-todo/panel/1', goals: [] })).toBeUndefined();
    expect(parseGoalsPayload({ schema: 'kc-goals/panel/2' })).toBeUndefined();
  });

  it('accepts a newer schema version, so a plugin upgrade does not hide goals', () => {
    const parsed = parseGoalsPayload(payload({ schema: 'kc-goals/panel/9' }));
    expect(parsed?.goals).toHaveLength(1);
  });

  it('drops an action with no instruction, because a button could not perform it', () => {
    const parsed = parseGoalsPayload(
      payload({ goals: [goal({ actions: [action({ instruction: '' }), action({ id: 'status' })] })] }),
    );
    expect(parsed?.goals[0]?.actions.map((a) => a.id)).toEqual(['status']);
  });

  it('ignores fields of the wrong type instead of throwing', () => {
    const parsed = parseGoalsPayload(
      payload({
        goals: [
          goal({
            criteria_detail: [criterion({ text: 42 }), criterion({ id: 'c9' })],
            options: [option({ value: null })],
            journal: [{ note: 7 }],
          }),
        ],
      }),
    );
    expect(parsed?.goals[0]?.criteria_detail.map((c) => c.id)).toEqual(['c9']);
    expect(parsed?.goals[0]?.options).toEqual([]);
    expect(parsed?.goals[0]?.journal).toEqual([]);
  });

  it('keeps an error-only payload so the reason can be shown', () => {
    const parsed = parseGoalsPayload(payload({ goals: [], error: 'the goal file is not valid JSON' }));
    expect(parsed?.error).toBe('the goal file is not valid JSON');
  });
});

describe('optionText', () => {
  it('renders zero as its meaning, not as a literal', () => {
    expect(optionText({ key: 'k', label: 'l', kind: 'number', value: 0, zero_means: 'no cap' })).toBe('no cap');
  });

  it('renders a number with its unit, and a switch as on/off', () => {
    expect(optionText({ key: 'k', label: 'l', kind: 'number', value: 5, unit: 'minutes' })).toBe('5 minutes');
    expect(optionText({ key: 'k', label: 'l', kind: 'switch', value: true })).toBe('on');
  });
});

describe('the registered renderer', () => {
  it('claims a goals payload and declines anything else', () => {
    expect(goalsPanelRenderer.matches(payload())).toBe(true);
    expect(goalsPanelRenderer.matches({ unrelated: true })).toBe(false);
    expect(goalsPanelRenderer.matches({ schema: 'kc-todo/panel/1', items: [] })).toBe(false);
  });

  it('is found under the hosted id', () => {
    registerPluginPanelRenderer(GOALS_PANEL_ID, goalsPanelRenderer);
    expect(findPluginPanelRenderer(panelTab())).toBe(goalsPanelRenderer);
  });
});

describe('GoalsPanel', () => {
  it('draws the goal, its criteria, the check verdict and the evidence', () => {
    render(<GoalsPanel data={payload()} panel={panelTab()} plugin="kc-goals" version="0.1.0" refresh={() => undefined} />);
    expect(screen.getByText('Ship the widget')).toBeTruthy();
    expect(screen.getByText('1/2 criteria')).toBeTruthy();
    expect(screen.getByText('builds')).toBeTruthy();
    expect(screen.getByText('cargo build --release')).toBeTruthy();
    // The check command and its "never run" verdict are visible without opening
    // anything, because that is the state that needs acting on.
    expect(screen.getByText('cargo test -q')).toBeTruthy();
    expect(screen.getByText('not run')).toBeTruthy();
    expect(screen.getByText('Next: do the work for `c2`')).toBeTruthy();
  });

  it('shows a paused goal’s reason and only the actions that apply', () => {
    const data = payload({
      goals: [
        goal({
          status: 'paused',
          pause_reason: 'the run was cancelled',
          actions: [action({ id: 'resume', label: 'Resume', kind: 'primary' })],
        }),
      ],
    });
    render(<GoalsPanel data={data} panel={panelTab()} plugin="kc-goals" version="0.1.0" refresh={() => undefined} />);
    expect(screen.getByText(/the run was cancelled/)).toBeTruthy();
    expect(screen.getByText('Resume')).toBeTruthy();
    expect(screen.queryByText('Pause')).toBeNull();
  });

  it('says how to start a goal when there is none', () => {
    render(
      <GoalsPanel
        data={payload({ goals: [], total: 0, open: 0 })}
        panel={panelTab()}
        plugin="kc-goals"
        version="0.1.0"
        refresh={() => undefined}
      />,
    );
    expect(screen.getByText(/No goals in this workspace/)).toBeTruthy();
  });

  it('posts an action’s instruction into the session', async () => {
    const refresh = vi.fn();
    render(<GoalsPanel data={payload()} panel={panelTab()} plugin="kc-goals" version="0.1.0" refresh={refresh} />);
    fireEvent.click(screen.getByText('Pause'));
    await waitFor(() => expect(sendMessage).toHaveBeenCalledTimes(1));
    expect(sendMessage.mock.calls[0]?.[0]).toBe('s_1');
    expect(sendMessage.mock.calls[0]?.[1]).toContain('plugin__kc-goals__goal_pause for g1');
    await waitFor(() => expect(screen.getByText('sent')).toBeTruthy());
    expect(refresh).toHaveBeenCalled();
  });

  it('queues the instruction when a run is already working', async () => {
    sendMessage.mockRejectedValueOnce(new ApiError('busy', 409, 'conflict' as never));
    render(<GoalsPanel data={payload()} panel={panelTab()} plugin="kc-goals" version="0.1.0" refresh={() => undefined} />);
    fireEvent.click(screen.getByText('Pause'));
    await waitFor(() => expect(sendQueuedMessage).toHaveBeenCalledTimes(1));
    expect(sendQueuedMessage.mock.calls[0]?.[1]).toBe('follow_up');
  });

  it('shows a failure instead of pretending the message was delivered', async () => {
    sendMessage.mockRejectedValueOnce(new Error('the server is not reachable'));
    render(<GoalsPanel data={payload()} panel={panelTab()} plugin="kc-goals" version="0.1.0" refresh={() => undefined} />);
    fireEvent.click(screen.getByText('Pause'));
    await waitFor(() => expect(screen.getByText('the server is not reachable')).toBeTruthy());
    expect(screen.queryByText('sent')).toBeNull();
  });

  it('does not send a disabled action', () => {
    const data = payload({ goals: [goal({ actions: [action({ id: 'complete', label: 'Complete', enabled: false })] })] });
    render(<GoalsPanel data={data} panel={panelTab()} plugin="kc-goals" version="0.1.0" refresh={() => undefined} />);
    const button = screen.getByText('Complete').closest('button');
    expect(button?.disabled).toBe(true);
    fireEvent.click(screen.getByText('Complete'));
    expect(sendMessage).not.toHaveBeenCalled();
  });

  it('renders an unparseable payload as an explanation, not a crash', () => {
    render(<GoalsPanel data={{ schema: 'something/else' }} panel={panelTab()} plugin="kc-goals" version="0.1.0" refresh={() => undefined} />);
    expect(screen.getByText(/does not understand its payload/)).toBeTruthy();
  });

  it('draws through PluginPanelView when the renderer is registered', async () => {
    registerPluginPanelRenderer(GOALS_PANEL_ID, goalsPanelRenderer);
    const store = await import('../state/store');
    store.pluginPanelData.value = {
      [GOALS_PANEL_ID]: { loading: false, data: payload(), title: 'Goals', error: null, fetchedAt: Date.now() },
    };
    const { PluginPanelView } = await import('./PluginPanelView');
    render(<PluginPanelView panel={panelTab()} />);
    expect(await screen.findByText('Ship the widget')).toBeTruthy();
    expect(screen.getByText('kc-goals v0.1.0')).toBeTruthy();
  });
});
