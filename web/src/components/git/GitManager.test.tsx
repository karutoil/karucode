import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, renderHook, screen, waitFor, within } from '@testing-library/preact';
import { GitManager } from './GitManager';
import { GitCommitBox } from './GitCommitBox';
import { useGitRepo } from './useGitRepo';
import type {
  GitBranchList,
  GitCommitDetail,
  GitCommitList,
  GitDiff,
  GitStashList,
  GitStatus,
} from '../../api/types';
import { jsonResponse, resetAppState } from '../../test/fixtures';
import * as store from '../../state/store';

const STATUS: GitStatus = {
  is_repo: true,
  branch: 'feat/git',
  detached: false,
  upstream: 'origin/feat/git',
  ahead: 2,
  behind: 0,
  error: null,
  truncated: false,
  files: [
    {
      path: 'src/plugins/loader.rs',
      orig_path: null,
      index: ' ',
      worktree: 'M',
      status: 'M',
      staged: false,
      unstaged: true,
      untracked: false,
      conflict: false,
    },
    {
      path: 'src/plugins/manifest.rs',
      orig_path: null,
      index: 'M',
      worktree: ' ',
      status: 'M',
      staged: true,
      unstaged: false,
      untracked: false,
      conflict: false,
    },
    {
      path: 'tests/plugin_lifecycle.rs',
      orig_path: null,
      index: '?',
      worktree: '?',
      status: '??',
      staged: false,
      unstaged: true,
      untracked: true,
      conflict: false,
    },
  ],
};

const BRANCHES: GitBranchList = {
  is_repo: true,
  branch: 'feat/git',
  detached: false,
  truncated: false,
  branches: [
    {
      name: 'feat/git',
      current: true,
      commit: 'abc1234',
      upstream: 'origin/feat/git',
      ahead: 2,
      behind: 0,
      timestamp: 1_700_000_000,
      subject: 'feat: git manager',
    },
    {
      name: 'main',
      current: false,
      commit: 'def5678',
      upstream: null,
      ahead: 0,
      behind: 0,
      timestamp: 1_699_000_000,
      subject: 'init',
    },
  ],
};

const LOG: GitCommitList = {
  is_repo: true,
  truncated: false,
  commits: [
    {
      id: 'abc1234aaa',
      short_id: 'abc1234',
      author: 'Test',
      email: 'test@example.com',
      timestamp: 1_700_000_000,
      parents: ['def5678bbb'],
      refs: 'HEAD -> feat/git',
      subject: 'feat: git manager',
    },
    {
      id: 'def5678bbb',
      short_id: 'def5678',
      author: 'Test',
      email: 'test@example.com',
      timestamp: 1_699_000_000,
      parents: [],
      refs: '',
      subject: 'init',
    },
  ],
};

const DETAIL: GitCommitDetail = {
  id: 'abc1234aaa',
  short_id: 'abc1234',
  author: 'Test',
  email: 'test@example.com',
  timestamp: 1_700_000_000,
  parents: ['def5678bbb'],
  refs: 'HEAD -> feat/git',
  subject: 'feat: git manager',
  body: 'Adds the source control manager.',
  files: [{ path: 'src/git.ts', additions: 4, deletions: 1, binary: false }],
  diff: ['diff --git a/src/git.ts b/src/git.ts', '@@ -1 +1 @@', '-old', '+new'].join('\n'),
  truncated: false,
};

const STASHES: GitStashList = { is_repo: true, truncated: false, stashes: [] };

const DIFF: GitDiff = {
  diff: [
    'diff --git a/src/plugins/loader.rs b/src/plugins/loader.rs',
    '--- a/src/plugins/loader.rs',
    '+++ b/src/plugins/loader.rs',
    '@@ -1,2 +1,3 @@',
    ' impl PluginRegistry {',
    '-    pub fn load(&mut self) {',
    '+    pub async fn load(&mut self, manifest: &PluginManifest) {',
    ' }',
  ].join('\n'),
  truncated: false,
};

interface Call {
  method: string;
  path: string;
  query: URLSearchParams;
  body: Record<string, unknown> | null;
}

interface MockOptions {
  status?: GitStatus;
  /** The status a mutation answers with (defaults to `status`). */
  statusAfter?: GitStatus;
  branches?: GitBranchList;
  log?: GitCommitList;
  stashes?: GitStashList;
  /** Fail the status probe with this message. */
  statusError?: string;
}

function installFetch(options: MockOptions = {}): Call[] {
  const calls: Call[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input), 'http://localhost');
    const method = (init?.method ?? 'GET').toUpperCase();
    const body = typeof init?.body === 'string' ? (JSON.parse(init.body) as Record<string, unknown>) : null;
    calls.push({ method, path: url.pathname, query: url.searchParams, body });

    if (options.statusError && url.pathname === '/api/git/status') {
      return jsonResponse({ error: { code: 'internal', message: options.statusError } }, 500);
    }
    if (method === 'GET') {
      if (url.pathname === '/api/git/status') return jsonResponse(options.status ?? STATUS);
      if (url.pathname === '/api/git/branches') return jsonResponse(options.branches ?? BRANCHES);
      if (url.pathname === '/api/git/log') return jsonResponse(options.log ?? LOG);
      if (url.pathname === '/api/git/stash') return jsonResponse(options.stashes ?? STASHES);
      if (url.pathname === '/api/git/diff') return jsonResponse(DIFF);
      if (url.pathname.startsWith('/api/git/commits/')) return jsonResponse(DETAIL);
    }
    if (method === 'POST') {
      const status = options.statusAfter ?? options.status ?? STATUS;
      const committed =
        url.pathname === '/api/git/commit'
          ? { id: 'abc1234aaa', short_id: 'abc1234', subject: 'feat: git manager' }
          : null;
      return jsonResponse({
        ok: true,
        message: `done: ${url.pathname}`,
        status,
        commit: committed,
      });
    }
    return jsonResponse({ error: { code: 'not_found', message: `unmocked ${method} ${url.pathname}` } }, 404);
  });
  vi.stubGlobal('fetch', fetchMock);
  return calls;
}

function posts(calls: Call[], path: string): Call[] {
  return calls.filter((call) => call.method === 'POST' && call.path === path);
}

function branchChip(): HTMLElement {
  const chip = document.querySelector<HTMLElement>('.gitm-branch');
  if (!chip) throw new Error('no branch chip');
  return chip;
}

beforeEach(() => {
  resetAppState();
  store.currentProjectId.value = null;
});

afterEach(() => {
  store.resetToastTimers();
  vi.unstubAllGlobals();
  resetAppState();
});

describe('GitManager — full-width source control view', () => {
  it('shows the branch, the counts and the three groups', async () => {
    installFetch();
    const { container } = render(<GitManager variant="page" />);

    expect(await screen.findByRole('region', { name: 'Staged' })).not.toBeNull();
    expect(document.querySelector('.gitm-branch-label')?.textContent).toBe('feat/git');
    expect(screen.getByText('3 changes · 1 staged · 1 unstaged · 1 untracked')).not.toBeNull();
    // The tracking counts are real numbers, not a fabricated "in sync".
    expect(container.querySelector('.gitm-branch-track')?.textContent).toBe('↑2');

    const staged = screen.getByRole('region', { name: 'Staged' });
    const changes = screen.getByRole('region', { name: 'Changes' });
    const untracked = screen.getByRole('region', { name: 'Untracked' });
    expect(within(staged).getByText('manifest.rs')).not.toBeNull();
    expect(within(changes).getByText('loader.rs')).not.toBeNull();
    expect(within(changes).getByText('src/plugins')).not.toBeNull();
    expect(within(untracked).getByText('plugin_lifecycle.rs')).not.toBeNull();
    // Colour is never the only signal: the badge letter is in the row.
    expect(within(changes).getByText('M')).not.toBeNull();
    expect(within(untracked).getByText('?')).not.toBeNull();
  });

  it('opens the patch in the main region and asks for that side of the diff', async () => {
    const calls = installFetch();
    render(<GitManager variant="page" />);

    const changes = await screen.findByRole('region', { name: 'Changes' });
    fireEvent.click(within(changes).getByRole('button', { name: 'Modified src/plugins/loader.rs' }));

    expect(await screen.findByText(/pub async fn load/)).not.toBeNull();
    const diffCall = calls.find((call) => call.path === '/api/git/diff');
    expect(diffCall?.query.get('path')).toBe('src/plugins/loader.rs');
    expect(diffCall?.query.get('staged')).toBe('false');
    // A selected row is marked in the list as well as in the pane.
    expect(containerRow('loader.rs').className).toContain('is-active');
  });

  it('asks for the staged side when the row comes from the staged group', async () => {
    const calls = installFetch();
    render(<GitManager variant="page" />);

    const staged = await screen.findByRole('region', { name: 'Staged' });
    fireEvent.click(within(staged).getByRole('button', { name: 'Modified src/plugins/manifest.rs' }));

    await waitFor(() => expect(calls.some((call) => call.path === '/api/git/diff')).toBe(true));
    const diffCall = calls.find((call) => call.path === '/api/git/diff');
    expect(diffCall?.query.get('staged')).toBe('true');
  });

  it('stages one file and applies the status the server answers with', async () => {
    const clean: GitStatus = { ...STATUS, files: [] };
    const calls = installFetch({ statusAfter: clean });
    const { container } = render(<GitManager variant="page" />);

    const changes = await screen.findByRole('region', { name: 'Changes' });
    fireEvent.click(within(changes).getByRole('button', { name: 'Stage src/plugins/loader.rs' }));

    await waitFor(() => expect(posts(calls, '/api/git/stage')).toHaveLength(1));
    expect(posts(calls, '/api/git/stage')[0]?.body).toEqual({ paths: ['src/plugins/loader.rs'] });
    // The mutation's response *is* the new state: no second probe needed.
    await waitFor(() =>
      expect(container.querySelector('.gitm-empty-title')?.textContent).toBe('Working tree clean'),
    );
    expect(screen.queryByRole('region', { name: 'Changes' })).toBeNull();
  });

  it('unstage from the staged group, and stage a whole group at once', async () => {
    const calls = installFetch();
    render(<GitManager variant="page" />);

    const staged = await screen.findByRole('region', { name: 'Staged' });
    fireEvent.click(within(staged).getByRole('button', { name: 'Unstage src/plugins/manifest.rs' }));
    await waitFor(() => expect(posts(calls, '/api/git/unstage')).toHaveLength(1));
    expect(posts(calls, '/api/git/unstage')[0]?.body).toEqual({ paths: ['src/plugins/manifest.rs'] });

    fireEvent.click(screen.getByRole('button', { name: 'Stage all Untracked' }));
    await waitFor(() => expect(posts(calls, '/api/git/stage')).toHaveLength(1));
    expect(posts(calls, '/api/git/stage')[0]?.body).toEqual({ paths: ['tests/plugin_lifecycle.rs'] });
  });

  it('gives coexisting page managers independent commit labels and drafts', async () => {
    installFetch();
    const { container } = render(
      <>
        <GitManager variant="page" />
        <GitManager variant="page" />
      </>,
    );

    expect(await screen.findAllByRole('region', { name: 'Staged' })).toHaveLength(2);
    const forms = Array.from(container.querySelectorAll<HTMLFormElement>('form.gitm-commit'));
    const messages = forms.map((form) => form.querySelector<HTMLTextAreaElement>('.gitm-message'));
    const labels = forms.map((form) => form.querySelector<HTMLLabelElement>('label[for]'));
    expect(forms).toHaveLength(2);
    expect(screen.getAllByRole('textbox', { name: 'Commit message' })).toHaveLength(2);

    expect(new Set(messages.map((message) => message?.id)).size).toBe(2);
    messages.forEach((message, index) => {
      expect(message?.id).not.toBe('');
      expect(labels[index]?.getAttribute('for')).toBe(message?.id);
      expect(labels[index]?.control).toBe(message);
      expect(document.getElementById(message!.id)).toBe(message);
    });

    fireEvent.input(messages[0]!, { target: { value: 'feat: only the first manager' } });
    expect(messages[0]?.value).toBe('feat: only the first manager');
    expect(messages[1]?.value).toBe('');
  });

  it('resets one mounted commit box when the project changes but branch stays main', async () => {
    const onCommit = vi.fn(async () => true);
    const common = {
      variant: 'page' as const,
      status: { ...STATUS, branch: 'main' },
      branch: 'main',
      busy: null,
      onCommit,
    };
    const view = render(<GitCommitBox {...common} projectId="p_a" />);
    const message = screen.getByRole('textbox', { name: 'Commit message' });
    fireEvent.input(message, { target: { value: 'project A draft' } });
    fireEvent.click(screen.getByRole('checkbox', { name: /amend/i }));

    view.rerender(<GitCommitBox {...common} projectId="p_b" />);
    await waitFor(() => expect((message as HTMLTextAreaElement).value).toBe(''));
    expect(screen.getByRole('textbox', { name: 'Commit message' })).toBe(message);
    expect((screen.getByRole('checkbox', { name: /amend/i }) as HTMLInputElement).checked).toBe(false);
  });

  it('clears drafts and selection across projects that both report main', async () => {
    installFetch({ status: { ...STATUS, branch: 'main' } });
    const view = render(<GitManager variant="page" />);
    const changes = await screen.findByRole('region', { name: 'Changes' });
    fireEvent.click(within(changes).getByRole('button', { name: 'Modified src/plugins/loader.rs' }));
    expect(await screen.findByText(/pub async fn load/)).not.toBeNull();
    const message = screen.getByRole('textbox', { name: 'Commit message' });
    fireEvent.input(message, { target: { value: 'belongs only to project A' } });
    fireEvent.click(screen.getByRole('checkbox', { name: /amend/i }));

    act(() => {
      store.currentProjectId.value = 'p_b';
    });
    view.rerender(<GitManager variant="page" />);

    await waitFor(() => expect((screen.getByRole('textbox', { name: 'Commit message' }) as HTMLTextAreaElement).value).toBe(''));
    expect((screen.getByRole('checkbox', { name: /amend/i }) as HTMLInputElement).checked).toBe(false);
    await waitFor(() => expect(screen.queryByText(/pub async fn load/)).toBeNull());
    expect(containerRow('loader.rs').className).not.toContain('is-active');
  });

  it('commits the staged files with the typed message', async () => {
    const calls = installFetch({ statusAfter: { ...STATUS, files: [] } });
    render(<GitManager variant="page" />);

    await screen.findByRole('region', { name: 'Staged' });
    const message = screen.getByRole('textbox', { name: 'Commit message' });
    const commit = screen.getByRole('button', { name: 'Commit' });
    expect((commit as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText('Write a commit message first')).not.toBeNull();

    fireEvent.input(message, { target: { value: 'feat: source control' } });
    await waitFor(() => expect((commit as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(commit);

    await waitFor(() => expect(posts(calls, '/api/git/commit')).toHaveLength(1));
    expect(posts(calls, '/api/git/commit')[0]?.body).toEqual({
      message: 'feat: source control',
      amend: false,
      // Something was staged, so the commit does not stage anything else.
      stage_all: false,
    });
    // The box clears on success, so the next commit starts empty.
    await waitFor(() => expect((message as HTMLTextAreaElement).value).toBe(''));
  });

  it('stages everything first when nothing is staged', async () => {
    const unstagedOnly: GitStatus = {
      ...STATUS,
      files: STATUS.files.filter((file) => !file.staged),
    };
    const calls = installFetch({ status: unstagedOnly });
    render(<GitManager variant="page" />);

    await screen.findByRole('region', { name: 'Changes' });
    const message = screen.getByRole('textbox', { name: 'Commit message' });
    fireEvent.input(message, { target: { value: 'all of it' } });
    await waitFor(() =>
      expect(
        screen.getByText('No staged changes — Commit stages every change first.'),
      ).not.toBeNull(),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Commit' }));

    await waitFor(() => expect(posts(calls, '/api/git/commit')).toHaveLength(1));
    expect(posts(calls, '/api/git/commit')[0]?.body).toMatchObject({ stage_all: true });
  });

  it('amends the last commit without a message', async () => {
    const calls = installFetch();
    render(<GitManager variant="page" />);

    await screen.findByRole('region', { name: 'Staged' });
    fireEvent.click(screen.getByRole('checkbox', { name: /amend/i }));
    const amend = screen.getByRole('button', { name: 'Amend commit' });
    expect((amend as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByText('The previous commit will be rewritten.')).not.toBeNull();
    fireEvent.click(amend);

    await waitFor(() => expect(posts(calls, '/api/git/commit')).toHaveLength(1));
    expect(posts(calls, '/api/git/commit')[0]?.body).toMatchObject({ message: '', amend: true });
  });

  it('commits with Ctrl/Cmd+Enter from the message box', async () => {
    const calls = installFetch({ statusAfter: { ...STATUS, files: [] } });
    render(<GitManager variant="page" />);

    await screen.findByRole('region', { name: 'Staged' });
    const message = screen.getByRole('textbox', { name: 'Commit message' });
    fireEvent.input(message, { target: { value: 'feat: via chord' } });
    await waitFor(() => expect((screen.getByRole('button', { name: 'Commit' }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.keyDown(message, { key: 'Enter', ctrlKey: true });

    await waitFor(() => expect(posts(calls, '/api/git/commit')).toHaveLength(1));
    expect(posts(calls, '/api/git/commit')[0]?.body).toMatchObject({ message: 'feat: via chord' });
  });

  it('asks before discarding, and only then sends confirm: true', async () => {
    const calls = installFetch();
    render(<GitManager variant="page" />);

    const changes = await screen.findByRole('region', { name: 'Changes' });
    fireEvent.click(within(changes).getByRole('button', { name: 'Discard changes in src/plugins/loader.rs' }));

    const dialog = await screen.findByRole('dialog');
    expect(dialog.textContent).toContain('Discard the changes in src/plugins/loader.rs?');
    // Nothing has been sent yet.
    expect(posts(calls, '/api/git/discard')).toHaveLength(0);

    fireEvent.click(within(dialog).getByRole('button', { name: 'Discard changes' }));
    await waitFor(() => expect(posts(calls, '/api/git/discard')).toHaveLength(1));
    expect(posts(calls, '/api/git/discard')[0]?.body).toEqual({
      paths: ['src/plugins/loader.rs'],
      staged: false,
      confirm: true,
    });
  });

  it('names the file and the consequence for an untracked discard', async () => {
    installFetch();
    render(<GitManager variant="page" />);

    const untracked = await screen.findByRole('region', { name: 'Untracked' });
    fireEvent.click(
      within(untracked).getByRole('button', { name: 'Discard changes in tests/plugin_lifecycle.rs' }),
    );

    const dialog = await screen.findByRole('dialog');
    expect(dialog.textContent).toContain('Delete tests/plugin_lifecycle.rs?');
    expect(dialog.textContent).toContain('deleted from disk');
    expect(within(dialog).getByRole('button', { name: 'Delete file' })).not.toBeNull();
  });

  it('shows the history and opens one commit in the main region', async () => {
    const calls = installFetch();
    render(<GitManager variant="page" />);

    // The full-width view has the room, so the history is already loaded.
    await waitFor(() => expect(calls.some((call) => call.path === '/api/git/log')).toBe(true));
    const commit = await screen.findByRole('button', { name: /feat: git manager/ });
    fireEvent.click(commit);

    expect(await screen.findByText('Adds the source control manager.')).not.toBeNull();
    const detail = calls.find((call) => call.path.startsWith('/api/git/commits/'));
    expect(detail?.path).toBe('/api/git/commits/abc1234aaa');
    expect(screen.getByText('src/git.ts')).not.toBeNull();
    expect(screen.getByText('+4')).not.toBeNull();
  });

  it('switches branch from the branches dialog', async () => {
    const calls = installFetch();
    render(<GitManager variant="page" />);

    await screen.findByRole('region', { name: 'Staged' });
    fireEvent.click(branchChip());
    const dialog = await screen.findByRole('dialog');
    expect(dialog.textContent).toContain('main');

    fireEvent.click(within(dialog).getByRole('button', { name: /^main/ }));
    await waitFor(() => expect(posts(calls, '/api/git/checkout')).toHaveLength(1));
    expect(posts(calls, '/api/git/checkout')[0]?.body).toEqual({ name: 'main' });
  });

  it('creates a branch, refusing a name git would reject', async () => {
    const calls = installFetch();
    render(<GitManager variant="page" />);

    await screen.findByRole('region', { name: 'Staged' });
    fireEvent.click(branchChip());
    const dialog = await screen.findByRole('dialog');
    const name = within(dialog).getByRole('textbox', { name: /new branch/i });
    const create = within(dialog).getByRole('button', { name: /create/i });

    fireEvent.input(name, { target: { value: 'bad name' } });
    await waitFor(() => expect((create as HTMLButtonElement).disabled).toBe(true));
    expect(within(dialog).getByRole('status').textContent).toContain('may not contain spaces');

    fireEvent.input(name, { target: { value: 'feat/next' } });
    await waitFor(() => expect((create as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(create);

    await waitFor(() => expect(posts(calls, '/api/git/branches')).toHaveLength(1));
    expect(posts(calls, '/api/git/branches')[0]?.body).toEqual({ name: 'feat/next', checkout: true });
  });

  it('explains a non-repository workspace instead of faking a tree', async () => {
    installFetch({ status: { is_repo: false, branch: null, error: null, files: [] } });
    render(<GitManager variant="page" />);

    expect(await screen.findByText('Not a git repository')).not.toBeNull();
    expect(document.body.textContent).toContain('git init');
    // No commit box for a tree that does not exist.
    expect(screen.queryByRole('textbox', { name: 'Commit message' })).toBeNull();
  });

  it('surfaces a failed probe with a retry that re-probes', async () => {
    let attempts = 0;
    vi.stubGlobal(
      'fetch',
      vi.fn(async (input: RequestInfo | URL) => {
        const url = String(input);
        if (url.startsWith('/api/git/status')) {
          attempts += 1;
          return jsonResponse({ error: { code: 'internal', message: 'git exploded' } }, 500);
        }
        return jsonResponse({ error: { code: 'not_found', message: url } }, 404);
      }),
    );
    render(<GitManager variant="page" />);

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('git exploded');
    fireEvent.click(within(alert).getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(attempts).toBe(2));
  });
});

describe('repository switching races', () => {
  it('ignores every late A response and cannot clear B’s mutation busy state', async () => {
    interface Pending {
      response: {
        promise: Promise<Response>;
        resolve: (value: Response) => void;
      };
      signal: AbortSignal | null;
    }
    const pending = new Map<string, Pending>();
    const requestKey = (project: string, method: string, path: string): string => `${project}:${method}:${path}`;
    const status = (project: string, suffix: string): GitStatus => ({
      ...STATUS,
      branch: 'main',
      upstream: null,
      ahead: project === 'p_b' ? 7 : 1,
      files: [{
        path: `${project}-${suffix}.txt`,
        orig_path: null,
        index: ' ',
        worktree: 'M',
        status: 'M',
        staged: false,
        unstaged: true,
        untracked: false,
        conflict: false,
      }],
    });
    const branches = (project: string): GitBranchList => ({
      ...BRANCHES,
      branch: 'main',
      branches: [{ ...BRANCHES.branches[1]!, current: true, subject: `${project} branch` }],
    });
    const answer = (project: string, method: string, path: string, body: unknown, statusCode = 200): void => {
      const found = pending.get(requestKey(project, method, path));
      if (!found) throw new Error(`missing deferred ${project} ${method} ${path}`);
      found.response.resolve(jsonResponse(body, statusCode));
    };

    vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), 'http://localhost');
      const project = url.searchParams.get('project') ?? 'p_a';
      const method = (init?.method ?? 'GET').toUpperCase();
      if (project === 'p_a' || method === 'POST') {
        let resolve!: (value: Response) => void;
        const promise = new Promise<Response>((onResolve) => { resolve = onResolve; });
        pending.set(requestKey(project, method, url.pathname), {
          response: { promise, resolve },
          signal: init?.signal ?? null,
        });
        return promise;
      }
      if (url.pathname === '/api/git/status') return Promise.resolve(jsonResponse(status(project, 'read')));
      if (url.pathname === '/api/git/branches') return Promise.resolve(jsonResponse(branches(project)));
      return Promise.resolve(jsonResponse({ error: { code: 'not_found', message: `unmocked ${url.pathname}` } }, 404));
    }));

    const hook = renderHook(({ project }: { project: string }) => useGitRepo(project), {
      initialProps: { project: 'p_a' },
    });
    let mutationA!: Promise<boolean>;
    act(() => {
      hook.result.current.loadHistory();
      hook.result.current.loadStashes();
      mutationA = hook.result.current.stage(['a.txt'], 'Staging A');
    });
    await waitFor(() => {
      expect(pending.has(requestKey('p_a', 'GET', '/api/git/status'))).toBe(true);
      expect(pending.has(requestKey('p_a', 'GET', '/api/git/branches'))).toBe(true);
      expect(pending.has(requestKey('p_a', 'GET', '/api/git/log'))).toBe(true);
      expect(pending.has(requestKey('p_a', 'GET', '/api/git/stash'))).toBe(true);
      expect(pending.has(requestKey('p_a', 'POST', '/api/git/stage'))).toBe(true);
    });
    expect(hook.result.current.busy).toBe('Staging A');

    act(() => hook.rerender({ project: 'p_b' }));
    await waitFor(() => expect(hook.result.current.status?.files[0]?.path).toBe('p_b-read.txt'));
    let mutationB!: Promise<boolean>;
    act(() => {
      mutationB = hook.result.current.stage(['b.txt'], 'Staging B');
    });
    await waitFor(() => expect(pending.has(requestKey('p_b', 'POST', '/api/git/stage'))).toBe(true));
    expect(hook.result.current.busy).toBe('Staging B');

    await act(async () => {
      answer('p_a', 'GET', '/api/git/status', status('p_a', 'late-read'));
      answer('p_a', 'GET', '/api/git/branches', branches('p_a'));
      answer('p_a', 'GET', '/api/git/log', {
        is_repo: true,
        truncated: false,
        commits: [{ ...LOG.commits[0]!, subject: 'late A history' }],
      });
      answer('p_a', 'GET', '/api/git/stash', {
        is_repo: true,
        truncated: false,
        stashes: [{ index: 0, ref: 'stash@{0}', message: 'late A stash', timestamp: 1 }],
      });
      answer('p_a', 'POST', '/api/git/stage', {
        ok: true,
        message: 'late A mutation',
        status: status('p_a', 'late-mutation'),
        commit: null,
      });
      await Promise.all(
        [...pending.entries()]
          .filter(([key]) => key.startsWith('p_a:'))
          .map(([, entry]) => entry.response.promise),
      );
      expect(await mutationA).toBe(false);
    });

    expect(hook.result.current.status?.files[0]?.path).toBe('p_b-read.txt');
    expect(hook.result.current.branches[0]?.subject).toBe('p_b branch');
    expect(hook.result.current.commits).toBeNull();
    expect(hook.result.current.stashes).toBeNull();
    expect(hook.result.current.busy).toBe('Staging B');
    expect(hook.result.current.actionError).toBeNull();
    expect(store.toasts.value.some((toast) => toast.message === 'late A mutation')).toBe(false);
    for (const [key, request] of pending) {
      if (key.startsWith('p_a:')) expect(request.signal?.aborted, key).toBe(true);
    }

    await act(async () => {
      answer('p_b', 'POST', '/api/git/stage', {
        ok: true,
        message: 'B complete',
        status: status('p_b', 'after-mutation'),
        commit: null,
      });
      expect(await mutationB).toBe(true);
    });
    await waitFor(() => expect(hook.result.current.status?.files[0]?.path).toBe('p_b-after-mutation.txt'));
    expect(hook.result.current.busy).toBeNull();
  });
});

describe('GitManager — inspector panel', () => {
  it('renders one column with the groups, the commit box and no history request yet', async () => {
    const calls = installFetch();
    const { container } = render(<GitManager variant="panel" />);

    expect(await screen.findByRole('region', { name: 'Staged' })).not.toBeNull();
    expect(container.querySelector('.gitm-panel')).not.toBeNull();
    expect(document.querySelector('.gitm-branch-label')?.textContent).toBe('feat/git');
    expect(container.querySelector('.gitm-page')).toBeNull();
    expect(screen.getByRole('textbox', { name: 'Commit message' })).not.toBeNull();
    expect(await screen.findByRole('region', { name: 'Staged' })).not.toBeNull();
    // The inspector opens on the status probe: the history is behind a disclosure.
    expect(calls.some((call) => call.path === '/api/git/log')).toBe(false);
  });

  it('opens the patch inline, below the list', async () => {
    installFetch();
    const { container } = render(<GitManager variant="panel" />);

    const changes = await screen.findByRole('region', { name: 'Changes' });
    fireEvent.click(within(changes).getByRole('button', { name: 'Modified src/plugins/loader.rs' }));

    expect(await screen.findByText(/pub async fn load/)).not.toBeNull();
    const detail = container.querySelector('.gitm-panel-detail');
    expect(detail).not.toBeNull();
    expect(within(detail as HTMLElement).getByText('src/plugins/loader.rs')).not.toBeNull();
  });

  it('loads the history when its disclosure opens', async () => {
    const calls = installFetch();
    render(<GitManager variant="panel" />);

    await screen.findByRole('region', { name: 'Staged' });
    fireEvent.click(screen.getByRole('button', { name: /History/ }));

    await waitFor(() => expect(calls.some((call) => call.path === '/api/git/log')).toBe(true));
    expect(await screen.findByText('feat: git manager')).not.toBeNull();
  });

  it('stays usable with a clean tree', async () => {
    installFetch({ status: { ...STATUS, files: [] } });
    render(<GitManager variant="panel" />);

    expect(await screen.findByText('Working tree clean')).not.toBeNull();
    expect(document.body.textContent).toContain('Nothing is staged or modified');
  });
});

/** The row whose text contains `name`, for class assertions. */
function containerRow(name: string): HTMLElement {
  const rows = Array.from(document.querySelectorAll<HTMLElement>('.gitm-row'));
  const found = rows.find((row) => row.textContent?.includes(name));
  if (!found) throw new Error(`no row for ${name}`);
  return found;
}