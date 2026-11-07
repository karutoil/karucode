/**
 * The control rail: the two lists (control chats vs project sessions), the
 * per-row actions and the session filters. Split out of `ControlPlane.test.tsx`
 * so a rail change never has to touch the overview's or the session's tests.
 *
 * The `vi.mock` blocks below are a per-file copy on purpose: `vi.mock` is
 * hoisted per test file, so a shared fixture module cannot register them.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import * as store from '../../state/store';
import * as actions from '../../state/actions';
import { resetControlPlane } from '../../state/controlPlane';
import { apiRequest } from '../../api/client';
import { getControlOverview, listAgents, listSessions } from '../../api/endpoints';
import { ControlPlane } from './ControlPlane';
import { chats, configWith, controlChat, overviewWith, projectSession, sessions } from './controlPlaneFixtures';

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

/* ------------------------------------------------------------- the two lists */

describe('the rail separates control chats from project sessions', () => {
  it('lists both groups, each from its own source', async () => {
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    const controlGroup = within(browser).getByLabelText('Control chats');
    expect(within(controlGroup).getByText('Control chat')).toBeTruthy();
    expect(within(controlGroup).getByText('Untitled chat')).toBeTruthy();
    const projectGroup = within(browser).getByLabelText('Sessions');
    expect(within(projectGroup).getByText('First')).toBeTruthy();
    expect(within(projectGroup).getByText('Second')).toBeTruthy();
  });

  it('never draws a control chat as a project row — by the control flag, not by id', async () => {
    // `c1` is in both reads (the id guard would have caught it). `c_flag` is only
    // in `/api/sessions`, flagged as a chat: the id set does not know it, so only
    // the flag can keep it out of the Sessions group.
    vi.mocked(listSessions).mockResolvedValue([
      ...sessions,
      ...chats,
      projectSession({ id: 'c_flag', title: 'Flag only chat', project_id: null, control: true }),
    ] as never);
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    const projectGroup = within(browser).getByLabelText('Sessions');
    expect(within(projectGroup).queryByText('Control chat')).toBeNull();
    expect(within(projectGroup).queryByText('Flag only chat')).toBeNull();
    // ...and the real chat is listed exactly once, in its own group.
    expect(within(browser).getAllByText('Control chat')).toHaveLength(1);
  });

  it('keeps the model off the row: the chat name is not a model label', async () => {
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    const row = within(group).getByRole('button', { name: /^Control chat/ });

    // The snapshot still carries a `model_label` for this chat, but the rail must
    // not draw it: a chat's model is not part of its identity, and the composer
    // is the one place the model is named.
    expect(row.textContent).not.toContain('gpt-4o-mini');
    expect(row.querySelector('[title^="Model:"]')).toBeNull();

    // The count is the workbench's `.badge`, not a private pill: the rail must
    // not draw a count in its own type or its own height.
    const agents = row.querySelector('.badge');
    expect(agents?.textContent).toBe('1/2 agents');
    expect(agents?.getAttribute('title')).toBe('1 subagent running · 2 total');
  });

  it('hides the badge a chat has nothing to say for', async () => {
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    const untitled = within(group).getByRole('button', { name: /^Untitled chat/ });
    // No subagents: a "0/0" chip would be noise, not information.
    expect(untitled.querySelector('.badge')).toBeNull();
  });
});

/* ---------------------------------------------------------------- row actions */

describe('row actions', () => {
  it('renames a chat from its kebab and re-reads the snapshot', async () => {
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: 'Actions for Control chat' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Rename' }));

    const dialog = await screen.findByRole('dialog');
    fireEvent.input(within(dialog).getByLabelText('Session title'), { target: { value: 'Release planning' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Rename' }));

    // The shared action owns the PATCH (and the optimistic row patch the
    // workspace rail needs); the plane only asks it and re-reads its snapshot.
    await waitFor(() => expect(actions.renameSessionById).toHaveBeenCalledWith('c1', 'Release planning'));
    // A rename is only real once the rail's own source says so.
    await waitFor(() => expect(getControlOverview).toHaveBeenCalledTimes(2));
  });

  it('deletes the open chat after a confirmation and returns to the overview', async () => {
    render(<ControlPlane />);
    const group = await screen.findByLabelText('Control chats');
    fireEvent.click(within(group).getByRole('button', { name: /^Control chat/ }));
    await screen.findByText('Normal chat');

    // A control chat is listed on the left while it is open, so its kebab is here.
    const rail = screen.getByLabelText('Sessions across projects');
    fireEvent.click(within(rail).getByRole('button', { name: 'Actions for Control chat' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete…' }));

    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText(/cannot be undone/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete session' }));

    await waitFor(() => expect(apiRequest).toHaveBeenCalledWith('/api/sessions/c1', { method: 'DELETE' }));
    await waitFor(() => expect(getControlOverview).toHaveBeenCalledTimes(2));
    // Deleting what was open leaves nothing to show, so the overview comes back.
    expect(await screen.findByText(/What are we building/)).toBeTruthy();
    // ...and the workspace store does not keep pointing at the deleted row.
    expect(store.currentSessionId.value).toBeNull();
  });
});

/* ------------------------------------------------------------ session filters */

describe('session filters', () => {
  it('keeps the chats when the project filter narrows to running only', async () => {
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    const filters = within(browser).getByRole('group', { name: 'Filter sessions' });
    fireEvent.click(within(filters).getByRole('button', { name: /Running/ }));
    const projectGroup = within(browser).getByLabelText('Sessions');
    expect(within(projectGroup).getByText('First')).toBeTruthy();
    expect(within(projectGroup).queryByText('Second')).toBeNull();
    // Filters are about project sessions; a chat is not one.
    expect(within(browser).getByLabelText('Control chats')).toBeTruthy();
  });

  it('keeps a distinct word on every state, not just a coloured dot', async () => {
    // The plane's status word used to be a private `.cp-state` span that could be
    // collapsed to `font-size: 0`, so "Finished", "Cancelled" and "No runs" all
    // rendered as the same grey dot. The word is always in the row - for the
    // settled states too, which the workbench's own rail leaves out - and it is
    // the shared `.session-state` this sheet inks.
    const quiet = [
      projectSession({ id: 's_done', title: 'Done one', last_run_status: 'completed' }),
      projectSession({ id: 's_cancelled', title: 'Cancelled one', last_run_status: 'cancelled' }),
      projectSession({ id: 's_never', title: 'Never ran' }),
    ];
    vi.mocked(listSessions).mockResolvedValue([...quiet, ...sessions, ...chats] as never);
    render(<ControlPlane />);

    const browser = await screen.findByLabelText('Sessions across projects');
    const row = (title: string): HTMLElement =>
      within(browser).getByRole('button', { name: new RegExp(`^${title}`) });

    expect(within(row('Done one')).getByText('Finished')).toBeTruthy();
    expect(within(row('Cancelled one')).getByText('Cancelled')).toBeTruthy();
    expect(within(row('Never ran')).getByText('No runs')).toBeTruthy();
    // The tooltip repeats the state for a reader who sees only the colour cue.
    expect(within(row('Done one')).getByTitle('Latest state: Finished')).toBeTruthy();
  });

  it('offers project mentions to the control chat', async () => {
    render(<ControlPlane />);
    const controlGroup = await screen.findByLabelText('Control chats');
    fireEvent.click(within(controlGroup).getByRole('button', { name: /^Control chat/ }));
    expect(await screen.findByText('Mentions: One, Two')).toBeTruthy();
  });
});

/* ------------------------------------------------- the workbench's vocabulary
 *
 * §5A of `docs/internal/DESIGN-SYSTEM.md`: a control-plane element that plays a
 * role the workbench already has must use the workbench's class for that role.
 * jsdom has no layout engine, so these tests check the *vocabulary* - the classes
 * and the DOM order that `styles/layout.css` and `components/SessionsRail.tsx`
 * define - and leave the measured pixels to `scripts/verify_control_parity.py`.
 */

/** The rail as rendered, with its rows. */
async function mountedRail(): Promise<{
  browser: HTMLElement;
  rows: Element[];
  controlGroup: HTMLElement;
  projectGroup: HTMLElement;
}> {
  render(<ControlPlane />);
  const browser = await screen.findByLabelText('Sessions across projects');
  const rows = [...browser.querySelectorAll('.session-row')];
  return {
    browser,
    rows,
    controlGroup: within(browser).getByLabelText('Control chats'),
    projectGroup: within(browser).getByLabelText('Sessions'),
  };
}

describe('the rail speaks the workbench’s session-row grammar', () => {
  it('draws every row with the shared classes, inside the shared list', async () => {
    const { browser, rows } = await mountedRail();
    // Two chats and two project sessions, all four in the shared row.
    expect(rows).toHaveLength(4);
    for (const row of rows) {
      expect(row.classList.contains('session-row')).toBe(true);
      // The quiet variant is what inks `.session-state` for a row in a rail.
      expect(row.classList.contains('session-row-quiet')).toBe(true);
      // `.session-list` is the row's flex parent: without it the rows do not lay
      // out at all, so a row outside one is a broken rail.
      expect(row.parentElement?.classList.contains('session-list')).toBe(true);
      expect(row.querySelector('.session-open')).toBeTruthy();
      expect(row.querySelector('.session-open .session-title')?.textContent?.trim()).toBeTruthy();
      expect(row.querySelector('.session-head .session-title')).toBeTruthy();
      // The title comes first in the head: the accessible name of a row is its
      // title, not its status.
      expect(row.querySelector('.session-head')?.firstElementChild?.className).toBe('session-title');
    }
    // The rows are inside the workbench's scrolling list column, not a private one.
    expect(browser.querySelector('.session-nav .session-list')).toBeTruthy();
  });

  it('adopts the workbench’s controls rather than re-drawing them', async () => {
    const { browser } = await mountedRail();
    for (const selector of [
      '.side-head', // rail head
      '.side-search', // search field
      '.session-chips', // filter row
      '.filter-chip', // filter pill
      '.session-nav', // the scrolling list column
      '.side-footer', // rail foot
      '.online-row', // its live row
      '.icon-button', // refresh + row kebab
      '.status-dot', // the status colour cue
      '.session-menu-wrap', // the row's kebab
      '.session-menu-trigger',
      '.badge', // the counts
      '.rail-avatar', // the project tile
      '.session-group-label', // the group headings
      '.button', // the New button
    ]) {
      expect(browser.querySelector(selector), selector).toBeTruthy();
    }
    // ...and the status dot carries the app's own tone vocabulary, which is what
    // `.status-dot.status-<tone>` in `base.css` paints.
    const dot = browser.querySelector('.status-dot');
    expect(dot?.className).toMatch(/status-(running|ok|warn|danger|muted)/);
  });

  it('paints the status word with the shared tone classes, never an empty one', async () => {
    const quietRows = [
      projectSession({ id: 's_done', title: 'Done one', last_run_status: 'completed' }),
      projectSession({ id: 's_failed', title: 'Failed one', last_run_status: 'failed' }),
      projectSession({ id: 's_cancelled', title: 'Cancelled one', last_run_status: 'cancelled' }),
      projectSession({ id: 's_never', title: 'Never ran' }),
    ];
    vi.mocked(listSessions).mockResolvedValue([...quietRows, ...sessions, ...chats] as never);
    const { rows } = await mountedRail();

    // Every tone the app has, spelled out: "Finished" (ok), "Failed" (danger),
    // "Cancelled"/"No runs" (muted) and "Running" (running).
    const tones = new Set(rows.map((row) =>
      [...(row.querySelector('.session-state')?.classList ?? [])]
        .find((name) => name.startsWith('session-state-'))));
    expect(tones).toEqual(new Set([
      'session-state-ok',
      'session-state-danger',
      'session-state-muted',
      'session-state-running',
    ]));

    for (const row of rows) {
      const word = row.querySelector('.session-state');
      // Never font-size 0 and never an empty span: three states that differ only
      // by colour are three states a reader cannot tell apart.
      expect(word?.textContent?.trim()).toBeTruthy();
      expect(word?.getAttribute('title')).toMatch(/^Latest state: /);
      // The word lives in the shared meta line, beside the meta dots.
      expect(word?.closest('.session-meta')).toBeTruthy();
    }
  });

  it('keeps the state word for a settled row, which the workbench rail omits', async () => {
    // Deliberate divergence, and the reason the plane cannot copy the workbench
    // row verbatim: across every project, "which of these needs me" is answered
    // by the words, and a settled row that says nothing is noise.
    const { projectGroup } = await mountedRail();
    const second = within(projectGroup).getByRole('button', { name: /^Second/ });
    expect(second.querySelector('.session-state')?.textContent).toBe('Finished');
  });

  it('marks the open row the way the workbench does', async () => {
    const { browser } = await mountedRail();
    fireEvent.click(within(browser).getByRole('button', { name: /^Second/ }));
    await waitFor(() =>
      expect(browser.querySelector('.session-row.is-active .session-title')?.textContent).toBe('Second'));
    expect(browser.querySelector('.session-row.is-active .session-open')?.getAttribute('aria-current'))
      .toBe('true');
    expect(browser.querySelectorAll('.session-row.is-active')).toHaveLength(1);
  });

  it('turns the row’s status rail rose for a chat waiting on a person', async () => {
    // `.session-row.needs::before` is the shared rule; the row state is the
    // plane's own fact (a chat parked on an approval question).
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({
      control: [controlChat({ id: 'c_wait', title: 'Waiting chat', pending_approvals: 1 })],
    }));
    const { controlGroup } = await mountedRail();
    const row = within(controlGroup).getByRole('button', { name: /^Waiting chat/ }).closest('li');
    expect(row?.classList.contains('needs')).toBe(true);
    expect(row?.querySelector('.session-state')?.textContent).toBe('Needs you');
  });
});

/* ------------------------------------------------------- icons, not glyphs */

/** Every character the old rail used as an icon. */
const GLYPHS = ['⟳', '⌕', '＋', '→', '✳', '↗', '←', '✕', '◈', '◆', '▼', '▸', '↻'];

describe('the rail draws icons, not text glyphs', () => {
  it('renders no stand-in glyph character anywhere in the rail', async () => {
    const { browser } = await mountedRail();
    const text = browser.textContent ?? '';
    for (const glyph of GLYPHS) expect(text, `${glyph} is a text glyph`).not.toContain(glyph);
  });

  it('renders no stand-in glyph in the empty-chat call to action either', async () => {
    // The `✳` and `→` used to live on this card, which only renders with no chats.
    vi.mocked(getControlOverview).mockResolvedValue(overviewWith({ control: [] }));
    const { browser, controlGroup } = await mountedRail();
    // Two controls offer a new chat: the group's New button and the card. The card
    // is the one with the spark and the chevron.
    const cta = within(controlGroup).getAllByRole('button', { name: /New control chat/ })
      .find((button) => button.classList.contains('cp-control-entry'));
    expect(cta).toBeTruthy();
    expect(cta?.querySelectorAll('svg').length).toBeGreaterThanOrEqual(2);
    const text = browser.textContent ?? '';
    for (const glyph of GLYPHS) expect(text, `${glyph} is a text glyph`).not.toContain(glyph);
  });

  it('puts an icon component where each glyph used to be', async () => {
    const { browser, rows } = await mountedRail();
    // Refresh, the search magnifier, the New button and every row kebab.
    expect(within(browser).getByRole('button', { name: 'Refresh sessions' })
      .querySelector('svg')).toBeTruthy();
    expect(browser.querySelector('.side-search svg')).toBeTruthy();
    expect(within(browser).getByRole('button', { name: 'New control chat' })
      .querySelector('svg')).toBeTruthy();
    expect(rows).toHaveLength(4);
    for (const row of rows) {
      const kebab = row.querySelector('.session-menu-wrap > button');
      // Wearing `.session-menu-trigger` is not enough: renaming its base class
      // would drop the shared 24px recipe and the hover-reveal rule while every
      // presence assertion here still passed. The role lives on the element.
      expect(kebab?.classList.contains('icon-button'), 'the kebab is an .icon-button').toBe(true);
      expect(kebab?.classList.contains('session-menu-trigger')).toBe(true);
      expect(kebab?.querySelector('svg')).toBeTruthy();
    }
    // The control chat's tile is a mark, not an icon: `k.` is the plane's own.
    expect(browser.querySelector('.cp-tile-control')?.textContent).toBe('k.');
  });
});

/* ------------------------------------------------------------------ footer */

describe('the rail foot', () => {
  it('counts the projects in the workbench’s foot, with the count spelled out', async () => {
    const { browser, projectGroup } = await mountedRail();
    const foot = browser.querySelector('.side-footer');
    expect(foot).toBeTruthy();
    expect(foot?.querySelector('.online-row')?.textContent).toBe('Live2 projects');
    expect(foot?.querySelector('.online-ind')?.className).toContain('status-ok');
    // The count badge is the shared pill, not a bare number in a span.
    // The count is the shared label's mono span, as `SessionsRail` draws its day
    // groups - not a second badge in a row of badges.
    expect(projectGroup.querySelector('.session-group-label span')?.textContent).toBe('2');
  });

  it('pluralises one project instead of saying "1 projects"', async () => {
    store.projects.value = [store.projects.value[0] as never];
    const { browser } = await mountedRail();
    expect(browser.querySelector('.online-row .right')?.textContent).toBe('1 project');
  });

  it('says the list is stale when the last read failed', async () => {
    vi.mocked(listSessions).mockRejectedValue(new Error('offline'));
    render(<ControlPlane />);
    const browser = await screen.findByLabelText('Sessions across projects');
    await waitFor(() =>
      expect(browser.querySelector('.online-row .right')?.textContent).toBe('2 projects'));
    expect(browser.querySelector('.online-row')?.textContent).toContain('Stale');
    expect(browser.querySelector('.online-ind')?.className).toContain('status-warn');
  });

  it('keeps a route to Settings, which the plane had none of', async () => {
    const { browser } = await mountedRail();
    fireEvent.click(within(browser).getByRole('button', { name: 'Settings' }));
    expect(actions.openSettings).toHaveBeenCalledTimes(1);
  });
});

/* -------------------------------------------------------------- row actions */

describe('the row menu keeps the rail’s keyboard contract', () => {
  it('owns its Escape and gives focus back to the kebab', async () => {
    const { browser } = await mountedRail();
    const trigger = within(browser).getByRole('button', { name: 'Actions for Control chat' });
    fireEvent.click(trigger);
    const menu = await within(browser).findByRole('menu');
    // The dialog layer defers to a control that owns Escape.
    expect(menu.hasAttribute('data-escape-local')).toBe(true);
    // Opening focuses the first item, as the workbench's menu does.
    expect(document.activeElement).toBe(within(menu).getByRole('menuitem', { name: 'Rename' }));

    fireEvent.keyDown(menu, { key: 'Escape' });
    expect(within(browser).queryByRole('menu')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it('walks the items with the arrow keys', async () => {
    const { browser } = await mountedRail();
    fireEvent.click(within(browser).getByRole('button', { name: 'Actions for Control chat' }));
    const menu = await within(browser).findByRole('menu');
    const items = within(menu).getAllByRole('menuitem');
    expect(items.map((item) => item.textContent)).toEqual(['Rename', 'Delete…']);

    const move = (key: string): void => {
      fireEvent.keyDown(document.activeElement ?? menu, { key });
    };
    // Down wraps forward, Up wraps back, Home/End jump — the same order the
    // kebab on a workbench row uses.
    move('ArrowDown');
    expect(document.activeElement).toBe(items[1]);
    move('ArrowDown');
    expect(document.activeElement).toBe(items[0]);
    move('ArrowUp');
    expect(document.activeElement).toBe(items[1]);
    move('Home');
    expect(document.activeElement).toBe(items[0]);
    move('End');
    expect(document.activeElement).toBe(items[1]);
  });
});
