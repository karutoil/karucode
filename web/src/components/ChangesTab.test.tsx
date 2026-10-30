import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import { ChangesTab } from './ChangesTab';
import type { GitStatus } from '../api/types';
import { jsonResponse, resetAppState } from '../test/fixtures';
import * as store from '../state/store';

const STATUS: GitStatus = {
  is_repo: true,
  branch: 'feat/plugin-lifecycle',
  error: null,
  files: [
    { path: 'src/plugins/loader.rs', status: 'M', staged: false },
    { path: 'src/plugins/manifest.rs', status: 'M', staged: true },
    { path: 'tests/plugin_lifecycle.rs', status: 'A', staged: false },
  ],
};

const DIFF = {
  path: 'src/plugins/loader.rs',
  staged: false,
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

function installFetch(status: GitStatus = STATUS) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
    const method = (init?.method ?? 'GET').toUpperCase();
    if (method === 'GET' && url.startsWith('/api/git/status')) return jsonResponse(status);
    if (method === 'GET' && url.startsWith('/api/git/diff')) return jsonResponse(DIFF);
    return jsonResponse({ error: { code: 'not_found', message: `unmocked ${method} ${url}` } }, 404);
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

describe('ChangesTab — changes view (page)', () => {
  beforeEach(() => {
    resetAppState();
    store.currentProjectId.value = null;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    resetAppState();
  });

  it("renders the mock's kicker, title, description and actions", async () => {
    installFetch();
    const { container } = render(<ChangesTab variant="page" />);

    expect(await screen.findByRole('heading', { name: 'Review changes' })).not.toBeNull();
    expect(screen.getByText('Source control')).not.toBeNull();
    expect(container.querySelector('.secondary-panel')).not.toBeNull();
    expect(container.querySelector('.panel-kicker')).not.toBeNull();
    expect(container.querySelector('.panel-title')).not.toBeNull();
    expect(container.querySelector('.panel-desc')).not.toBeNull();

    // The branch is real state, and the description names the file count.
    expect(await screen.findByText('feat/plugin-lifecycle')).not.toBeNull();
    expect(screen.getByText(/3 modified files in/)).not.toBeNull();

    const actions = container.querySelector('.panel-actions');
    expect(actions).not.toBeNull();
    expect(within(actions as HTMLElement).getByText(/3 modified/)).not.toBeNull();
    expect(within(actions as HTMLElement).getByRole('button', { name: /Refresh status/ })).not.toBeNull();
  });

  it('lists one card per changed file and reveals the diff on demand', async () => {
    const fetchMock = installFetch();
    const { container } = render(<ChangesTab variant="page" />);

    await waitFor(() => expect(container.querySelectorAll('.panel-change-file')).toHaveLength(3));
    const cards = [...container.querySelectorAll('.panel-change-file')];
    expect(cards[0]?.textContent).toContain('src/plugins/loader.rs');
    expect(cards[1]?.textContent).toContain('src/plugins/manifest.rs');
    expect(cards[2]?.textContent).toContain('tests/plugin_lifecycle.rs');

    const first = cards[0] as HTMLElement;
    const toggle = within(first).getByRole('button', { name: /loader\.rs/ });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');

    fireEvent.click(toggle);
    expect(await within(first).findByText(/pub async fn load/)).not.toBeNull();
    expect(toggle.getAttribute('aria-expanded')).toBe('true');

    const diffCalls = fetchMock.mock.calls.filter(([input]) =>
      String(input).startsWith('/api/git/diff'),
    );
    expect(diffCalls).toHaveLength(1);
    expect(String(diffCalls[0]?.[0])).toContain('path=src%2Fplugins%2Floader.rs');
  });

  it('explains a clean tree and a non-git workspace instead of faking rows', async () => {
    installFetch({ ...STATUS, files: [] });
    const clean = render(<ChangesTab variant="page" />);
    expect(await screen.findByText('Working tree clean')).not.toBeNull();
    expect(clean.container.querySelectorAll('.panel-change-file')).toHaveLength(0);
    clean.unmount();

    vi.unstubAllGlobals();
    installFetch({ is_repo: false, branch: null, error: null, files: [] });
    render(<ChangesTab variant="page" />);
    expect(await screen.findByText('Not a git repository')).not.toBeNull();
  });

  it('surfaces a failed status probe with a retry', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => jsonResponse({ error: { code: 'boom', message: 'git exploded' } }, 500)),
    );
    render(<ChangesTab variant="page" />);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('git exploded');
    expect(within(alert).getByRole('button', { name: 'Retry' })).not.toBeNull();
  });

  it('refetches when the project changes instead of showing the previous one', async () => {
    // The page variant stays mounted across a project switch, so the effect has
    // to depend on the project or the old files and diffs linger.
    store.currentProjectId.value = 'p_one';
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.startsWith('/api/git/status')) {
        const project = new URL(url, 'http://x').searchParams.get('project');
        return jsonResponse(
          project === 'p_two'
            ? {
                is_repo: true,
                branch: 'other-branch',
                error: null,
                files: [{ path: 'src/other.rs', status: 'M', staged: false }],
              }
            : STATUS,
        );
      }
      if (url.startsWith('/api/git/diff')) return jsonResponse(DIFF);
      return jsonResponse({ error: { code: 'not_found', message: url } }, 404);
    });
    vi.stubGlobal('fetch', fetchMock);

    const { container } = render(<ChangesTab variant="page" />);
    await waitFor(() => expect(container.querySelectorAll('.panel-change-file')).toHaveLength(3));
    expect(screen.getByText('feat/plugin-lifecycle')).not.toBeNull();

    // Switch project: the panel must follow, not keep the old working tree.
    store.currentProjectId.value = 'p_two';

    await waitFor(() => expect(screen.getByText('other-branch')).not.toBeNull());
    expect(container.querySelectorAll('.panel-change-file')).toHaveLength(1);
    expect(container.textContent).toContain('src/other.rs');
    expect(container.textContent).not.toContain('src/plugins/loader.rs');

    const statusCalls = fetchMock.mock.calls
      .map(([input]) => String(input))
      .filter((url) => url.startsWith('/api/git/status'));
    expect(statusCalls.some((url) => url.includes('project=p_two'))).toBe(true);
  });
});

describe('ChangesTab — inspector panel', () => {
  beforeEach(() => {
    resetAppState();
    store.currentProjectId.value = null;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    resetAppState();
  });

  it('renders metric cards and the mock\u2019s change rows', async () => {
    installFetch();
    const { container } = render(<ChangesTab />);

    await waitFor(() => expect(container.querySelectorAll('.change-row')).toHaveLength(3));
    expect(container.querySelectorAll('.metric')).toHaveLength(2);
    expect(container.querySelector('.metric-num')?.textContent).toContain('3');
    expect(screen.getByText('Changed files')).not.toBeNull();

    const first = container.querySelector('.change-row') as HTMLElement;
    expect(first.querySelector('.file-logo')?.textContent).toBe('rs');
    expect(first.querySelector('.change-name')?.textContent).toBe('loader.rs');
    expect(first.querySelector('.change-dir')?.textContent).toBe('src/plugins');
    expect(first.querySelector('.change-status-label')?.textContent).toContain('modified');
    // Staged state is spelled out rather than left to the colour.
    expect(container.textContent).toContain('modified · staged');
  });

  it('opens the inline diff for the selected row', async () => {
    installFetch();
    const { container } = render(<ChangesTab />);

    await waitFor(() => expect(container.querySelectorAll('.change-row')).toHaveLength(3));
    fireEvent.click(container.querySelector('.change-row') as HTMLElement);

    expect(await screen.findByText(/pub async fn load/)).not.toBeNull();
    expect(container.querySelector('.git-diff')).not.toBeNull();
    const diffCard = container.querySelector('.git-diff') as HTMLElement;
    expect(within(diffCard).getAllByText(/loader\.rs/).length).toBeGreaterThan(0);
  });
});
