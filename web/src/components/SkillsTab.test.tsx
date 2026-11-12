import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import type { SkillListResponse, SkillSummary } from '../api/types';
import { SkillsTab } from './SkillsTab';
import { resetSkillsForTests } from '../state/skills';
import * as store from '../state/store';
import { errorResponse, jsonResponse, resetAppState } from '../test/fixtures';

/**
 * The Skills tab, exercised through the real `api/endpoints` client against a
 * stubbed `fetch` (the way `AgentsTab.test.tsx` does it). That keeps the URL,
 * the method and the JSON body under test rather than a mock's argument order:
 * `PATCH /api/skills/release` with `{"enabled":false}` is what the verifier's
 * browser script will see too.
 */

interface Call {
  path: string;
  url: string;
  method: string;
  body: Record<string, unknown> | undefined;
}

let calls: Call[] = [];
/** What `GET /api/skills` currently answers; a test may replace it wholesale. */
let data: SkillListResponse;
/** What `PATCH /api/config` reports back as the stored `[skills]` section. */
let stored: Record<string, unknown>;

function skill(overrides: Partial<SkillSummary> & Pick<SkillSummary, 'id'>): SkillSummary {
  return {
    name: overrides.id,
    description: `Use ${overrides.id} when the task matches.`,
    source: 'workspace',
    path: `/ws/.karucode/skills/${overrides.id}/SKILL.md`,
    enabled: true,
    custom: false,
    ...overrides,
  };
}

function payload(overrides: Partial<SkillListResponse> = {}): SkillListResponse {
  return {
    enabled: true,
    auto_detect: true,
    directories: [],
    prompt: true,
    prompt_max_chars: 4000,
    max_skills: 64,
    truncated: false,
    roots: [
      {
        path: '/ws/.karucode/skills',
        source: 'workspace',
        exists: true,
        configured: false,
        count: 2,
      },
      {
        path: '/home/me/.karucode/skills',
        source: 'user',
        exists: false,
        configured: false,
        count: 0,
      },
      {
        path: '/home/me/my-skills',
        source: 'config',
        exists: true,
        configured: true,
        count: 1,
      },
    ],
    skills: [
      skill({ id: 'release', source: 'workspace' }),
      skill({
        id: 'house-style',
        source: 'custom',
        custom: true,
        path: null,
        description: 'Match the house component style.',
      }),
    ],
    ...overrides,
  };
}

const routes = {
  list: (): Response => jsonResponse(data),
  detail: (id: string): Response => {
    const row = data.skills.find((entry) => entry.id === id);
    return jsonResponse({ ...row, body: `# ${id}\n\nThe body of ${id}.` });
  },
  create: (body: Record<string, unknown> | undefined): Response =>
    jsonResponse(
      skill({
        id: String(body?.name ?? 'new-skill'),
        source: 'custom',
        custom: true,
        path: null,
        description: String(body?.description ?? ''),
      }),
      201,
    ),
  patchSkill: (id: string, body: Record<string, unknown> | undefined): Response => {
    const row = data.skills.find((entry) => entry.id === id);
    return jsonResponse({ ...row, ...body });
  },
  remove: (_id: string): Response => new Response(null, { status: 204 }),
  patchConfig: (body: Record<string, unknown> | undefined): Response =>
    jsonResponse({ ...store.config.value, skills: { ...stored, ...(body?.skills ?? {}) } }),
};

function handle(call: Call): Response {
  if (call.path === '/api/skills' && call.method === 'GET') return routes.list();
  if (call.path === '/api/skills' && call.method === 'POST') return routes.create(call.body);
  if (call.path === '/api/config' && call.method === 'PATCH') return routes.patchConfig(call.body);
  const match = /^\/api\/skills\/([^/]+)$/.exec(call.path);
  if (match) {
    const id = decodeURIComponent(match[1] ?? '');
    if (call.method === 'GET') return routes.detail(id);
    if (call.method === 'PATCH') return routes.patchSkill(id, call.body);
    if (call.method === 'DELETE') return routes.remove(id);
  }
  return errorResponse('not_found', `no route for ${call.method} ${call.path}`, 404);
}

function stubFetch(): void {
  calls = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      const body =
        typeof init?.body === 'string' ? (JSON.parse(init.body) as Record<string, unknown>) : undefined;
      const call: Call = {
        url,
        path: url.split('?')[0] ?? url,
        method: init?.method ?? 'GET',
        body,
      };
      calls.push(call);
      return handle(call);
    }),
  );
}

const writes = (): Call[] => calls.filter((call) => call.method !== 'GET');
const listReads = (): Call[] =>
  calls.filter((call) => call.method === 'GET' && call.path === '/api/skills');

async function renderTab(): Promise<void> {
  render(<SkillsTab />);
  await waitFor(() => expect(listReads().length).toBeGreaterThan(0));
  // The first read has resolved by the time the count is a number.
  await waitFor(() => expect(screen.getByTestId('skills-count').textContent).not.toContain('loading'));
}

function rowFor(id: string): HTMLElement {
  const row = document.querySelector<HTMLElement>(`[data-testid="skill-row"][data-skill-id="${id}"]`);
  if (!row) throw new Error(`no row for ${id}`);
  return row;
}

describe('SkillsTab', () => {
  beforeEach(() => {
    resetAppState();
    resetSkillsForTests();
    store.currentProjectId.value = 'p_1';
    data = payload();
    stored = { ...data };
    stubFetch();
  });

  afterEach(() => {
    resetAppState();
    resetSkillsForTests();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('lists every skill with its source badge and the count', async () => {
    await renderTab();

    expect(screen.getByTestId('skills-count').textContent).toContain('2 skills');
    const rows = [...document.querySelectorAll('[data-testid="skill-row"]')];
    expect(rows.map((row) => row.getAttribute('data-skill-id'))).toEqual(['release', 'house-style']);
    expect(rowFor('release').getAttribute('data-source')).toBe('workspace');
    expect(rowFor('release').getAttribute('data-enabled')).toBe('true');
    // `data-source` is the API's spelling; the badge is the label a person reads.
    expect(rowFor('house-style').getAttribute('data-source')).toBe('custom');
    expect(within(rowFor('release')).getByText('workspace')).not.toBeNull();
    expect(within(rowFor('house-style')).getByText('added')).not.toBeNull();
    expect(rowFor('release').textContent).toContain('Use release when the task matches.');
    // The file it came from, and the one that has no file at all.
    expect(rowFor('release').textContent).toContain('/ws/.karucode/skills/release/SKILL.md');
    expect(rowFor('house-style').textContent).toContain('Stored in config');

    // One read on open, and nothing else: the tab does not poll.
    expect(calls).toHaveLength(1);
    expect(calls[0]?.url).toBe('/api/skills?project=p_1');
  });

  it('toggles one skill with PATCH and takes the server’s answer', async () => {
    await renderTab();

    fireEvent.click(within(rowFor('release')).getByTestId('skill-toggle'));

    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(writes()[0]).toMatchObject({
      method: 'PATCH',
      body: { enabled: false },
    });
    expect(writes()[0]?.path).toBe('/api/skills/release');

    // The row reflects the stored value, and the switch says the same thing.
    await waitFor(() => expect(rowFor('release').getAttribute('data-enabled')).toBe('false'));
    expect(
      within(rowFor('release')).getByTestId('skill-toggle').getAttribute('aria-checked'),
    ).toBe('false');
    expect(screen.getByTestId('skills-count').textContent).toContain('1 off');
    // The other row is untouched.
    expect(rowFor('house-style').getAttribute('data-enabled')).toBe('true');
  });

  it('adds a skill with POST and re-reads the list', async () => {
    await renderTab();

    fireEvent.input(screen.getByTestId('skill-add-name'), { target: { value: 'deploy' } });
    fireEvent.input(screen.getByTestId('skill-add-description'), {
      target: { value: 'Ship the current build.' },
    });
    fireEvent.input(screen.getByTestId('skill-add-body'), {
      target: { value: '# Deploy\n\n1. Tag it.' },
    });
    fireEvent.click(screen.getByTestId('skill-add-submit'));

    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(writes()[0]).toMatchObject({
      method: 'POST',
      path: '/api/skills',
      body: {
        name: 'deploy',
        description: 'Ship the current build.',
        body: '# Deploy\n\n1. Tag it.',
      },
    });
    // The refresh is what puts the new row on screen (the server owns the id).
    await waitFor(() => expect(listReads()).toHaveLength(2));
  });

  it('refuses an empty body before any request', async () => {
    await renderTab();

    fireEvent.input(screen.getByTestId('skill-add-name'), { target: { value: 'empty' } });
    fireEvent.click(screen.getByTestId('skill-add-submit'));

    expect(screen.getByTestId('skill-add-error').textContent).toContain('needs a body');
    // Nothing was sent: not a POST, and no refresh either.
    expect(writes()).toHaveLength(0);
    expect(listReads()).toHaveLength(1);
    // A body of spaces is still empty.
    fireEvent.input(screen.getByTestId('skill-add-body'), { target: { value: '   ' } });
    fireEvent.click(screen.getByTestId('skill-add-submit'));
    expect(writes()).toHaveLength(0);
  });

  it('shows the server’s 400 when a delete is refused, and offers no delete for a file', async () => {
    const refusal =
      'release is file-backed (/ws/.karucode/skills/release/SKILL.md): edit or delete the file, ' +
      'or turn it off with PATCH instead.';
    routes.remove = () => errorResponse('bad_request', refusal, 400);

    await renderTab();

    fireEvent.click(within(rowFor('house-style')).getByTestId('skill-delete'));

    const alert = await screen.findByTestId('skills-error');
    expect(alert.textContent).toBe(refusal);
    expect(alert.getAttribute('role')).toBe('alert');
    // The list is still the server's: the row was not removed locally.
    expect(rowFor('house-style')).not.toBeNull();

    // A file-backed skill has no Delete at all, and the row says why.
    expect(within(rowFor('release')).queryByTestId('skill-delete')).toBeNull();
    expect(rowFor('release').textContent).toContain('Delete is refused');
    expect(rowFor('release').textContent).toContain('/ws/.karucode/skills/release/SKILL.md');
  });

  it('saves the options through PATCH /api/config and keeps what was stored', async () => {
    await renderTab();

    // The server clamps the budget to what it is willing to store; the box must
    // show the stored value, not the number that was typed.
    routes.patchConfig = (body) =>
      jsonResponse({
        skills: {
          ...stored,
          ...(body?.skills ?? {}),
          prompt_max_chars: 4000,
        },
      });

    fireEvent.click(screen.getByTestId('skills-autodetect-toggle'));
    fireEvent.input(screen.getByTestId('skills-prompt-max'), { target: { value: '99999' } });
    fireEvent.input(screen.getByTestId('skills-max-skills'), { target: { value: '8' } });
    fireEvent.click(screen.getByTestId('skills-save-options'));

    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(writes()[0]).toMatchObject({
      method: 'PATCH',
      path: '/api/config',
      body: {
        skills: {
          auto_detect: false,
          prompt: true,
          prompt_max_chars: 99999,
          max_skills: 8,
        },
      },
    });

    await waitFor(() =>
      expect((screen.getByTestId('skills-prompt-max') as HTMLInputElement).value).toBe('4000'),
    );
    expect((screen.getByTestId('skills-max-skills') as HTMLInputElement).value).toBe('8');
    expect(
      screen.getByTestId('skills-autodetect-toggle').getAttribute('aria-checked'),
    ).toBe('false');
    // Changing what is scanned makes the root list stale, and the tab says so
    // instead of re-reading behind the person's back.
    expect(screen.getByTestId('skills-roots-stale')).not.toBeNull();
  });

  it('saves the master switch on its own', async () => {
    await renderTab();

    fireEvent.click(screen.getByTestId('skills-master-toggle'));

    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(writes()[0]).toMatchObject({
      method: 'PATCH',
      path: '/api/config',
      body: { skills: { enabled: false } },
    });
    await waitFor(() =>
      expect(screen.getByTestId('skills-master-toggle').getAttribute('aria-checked')).toBe('false'),
    );
  });

  it('adds and removes an extra folder', async () => {
    await renderTab();

    fireEvent.input(screen.getByTestId('skills-dir-input'), {
      target: { value: '/home/me/my-skills' },
    });
    fireEvent.click(screen.getByTestId('skills-dir-add'));

    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(writes()[0]).toMatchObject({
      method: 'PATCH',
      path: '/api/config',
      body: { skills: { directories: ['/home/me/my-skills'] } },
    });

    const dirRow = await screen.findByTestId('skills-dir-row');
    expect(dirRow.getAttribute('data-dir')).toBe('/home/me/my-skills');
    fireEvent.click(within(dirRow).getByTestId('skills-dir-remove'));

    await waitFor(() => expect(writes()).toHaveLength(2));
    expect(writes()[1]).toMatchObject({
      method: 'PATCH',
      path: '/api/config',
      body: { skills: { directories: [] } },
    });
  });

  it('shows a root that was not found distinctly from an empty one', async () => {
    await renderTab();

    const roots = [...document.querySelectorAll<HTMLElement>('[data-testid="skills-root"]')];
    expect(roots).toHaveLength(3);
    expect(roots[0]?.getAttribute('data-exists')).toBe('true');
    expect(roots[0]?.getAttribute('data-source')).toBe('workspace');
    expect(roots[0]?.textContent).toContain('2 skills');
    expect(roots[1]?.getAttribute('data-exists')).toBe('false');
    expect(roots[1]?.textContent).toContain('not found');
    expect(roots[1]?.textContent).not.toContain('0 skills');
    expect(roots[1]?.textContent).toContain('/home/me/.karucode/skills');
    // A folder added through `skills.directories` carries the API's `config`
    // spelling and the `configured` label the tool's copy uses.
    expect(roots[2]?.getAttribute('data-source')).toBe('config');
    expect(roots[2]?.textContent).toContain('configured');
    expect(roots[2]?.textContent).toContain('1 skill');
  });

  it('says the feature is off and still shows the list', async () => {
    data = payload({ enabled: false });
    stored = { ...data };
    await renderTab();

    const banner = screen.getByTestId('skills-master-off');
    expect(banner.textContent).toContain('Skills are off');
    expect(banner.textContent).toContain('skill');
    expect(banner.textContent).toContain('tool refuses');
    expect(screen.getByTestId('skills-master-toggle').getAttribute('aria-checked')).toBe('false');
    // Nothing is hidden: the master switch is a state, not a reason to blank the panel.
    expect(document.querySelectorAll('[data-testid="skill-row"]')).toHaveLength(2);
    expect(screen.getByTestId('skill-add-submit')).not.toBeNull();
  });

  it('mirrors the API’s source spelling on the row and labels it for a person', async () => {
    data = payload({
      skills: [skill({ id: 'deploy', source: 'config', path: '/home/me/my-skills/deploy.md' })],
    });
    await renderTab();

    const row = rowFor('deploy');
    expect(row.getAttribute('data-source')).toBe('config');
    expect(within(row).getByText('configured')).not.toBeNull();
    expect(within(row).queryByText('config')).toBeNull();
  });

  it('explains that there are no skills and names the first workspace root', async () => {
    data = payload({
      skills: [],
      roots: [
        {
          path: '/ws/.karucode/skills',
          source: 'workspace',
          exists: true,
          configured: false,
          count: 0,
        },
      ],
    });
    await renderTab();

    expect(screen.getByText('No skills found')).not.toBeNull();
    expect(screen.getByTestId('skills-panel').textContent).toContain('/ws/.karucode/skills');
    expect(screen.getByTestId('skills-count').textContent).toContain('0 skills');
  });

  it('shows the body the server returned and refuses nothing about it', async () => {
    await renderTab();

    fireEvent.click(within(rowFor('release')).getByTestId('skill-view'));

    const body = await screen.findByTestId('skill-body');
    expect(body.textContent).toContain('# release');
    expect(body.textContent).toContain('The body of release.');
    expect(calls.some((call) => call.method === 'GET' && call.path === '/api/skills/release')).toBe(
      true,
    );
  });

  it('edits a custom skill with PATCH and re-reads the list', async () => {
    await renderTab();

    fireEvent.click(within(rowFor('house-style')).getByTestId('skill-edit'));
    const bodyField = await screen.findByTestId('skill-edit-body');

    fireEvent.input(bodyField, { target: { value: '# House style\n\nUse the tokens.' } });
    fireEvent.input(screen.getByTestId('skill-edit-description'), {
      target: { value: 'Match the house style.' },
    });
    fireEvent.click(screen.getByTestId('skill-edit-save'));

    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(writes()[0]).toMatchObject({
      method: 'PATCH',
      path: '/api/skills/house-style',
      body: {
        name: 'house-style',
        description: 'Match the house style.',
        body: '# House style\n\nUse the tokens.',
      },
    });
    await waitFor(() => expect(listReads()).toHaveLength(2));
  });

  it('keeps the add form when the server refuses the name', async () => {
    routes.create = () =>
      errorResponse('conflict', 'a skill with id release already exists', 409);
    await renderTab();

    fireEvent.input(screen.getByTestId('skill-add-name'), { target: { value: 'release' } });
    fireEvent.input(screen.getByTestId('skill-add-body'), { target: { value: '# Again' } });
    fireEvent.click(screen.getByTestId('skill-add-submit'));

    const alert = await screen.findByTestId('skills-error');
    expect(alert.textContent).toBe('a skill with id release already exists');
    // The draft survives: the person would otherwise retype the body.
    expect((screen.getByTestId('skill-add-body') as HTMLTextAreaElement).value).toBe('# Again');
  });

  it('offers no controls when the route is missing on this server', async () => {
    routes.list = () => errorResponse('not_found', 'no route for GET /api/skills', 404);
    render(<SkillsTab />);

    const alert = await screen.findByTestId('skills-error');
    expect(alert.textContent).toBe('no route for GET /api/skills');
    // The panel still renders its frame, with an explanation rather than a blank.
    expect(screen.getByTestId('skills-panel')).not.toBeNull();
    expect(screen.getByText('Skills are not available')).not.toBeNull();
  });
});