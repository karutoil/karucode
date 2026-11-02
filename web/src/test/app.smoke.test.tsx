import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import { App } from '../app';
import {
  configFixture,
  detailFixture,
  jsonResponse,
  resetAppState,
  summaryFixture,
} from './fixtures';
import type { Health } from '../api/types';
import * as store from '../state/store';

const healthFixture: Health = {
  status: 'ok',
  version: '0.1.0',
  uptime_secs: 12,
  workspace: '/home/me/project',
  projects: [],
  auth_required: false,
  provider_configured: true,
  db: 'ok',
};

function openEventStream(): Response {
  const stream = new ReadableStream<Uint8Array>({
    start() {
      // Keep the stream open: the smoke test only needs the initial render.
    },
  });
  return new Response(stream, {
    status: 200,
    headers: { 'content-type': 'text/event-stream' },
  });
}

function installFetch() {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
    void init;
    if (url.startsWith('/api/events')) return openEventStream();
    if (url === '/api/health') return jsonResponse(healthFixture);
    if (url === '/api/config') return jsonResponse(configFixture);
    if (url === '/api/projects')
      return jsonResponse({
        projects: [
          {
            id: 'p_one',
            name: 'project',
            path: '/home/me/project',
            created_at: 1,
            updated_at: 1,
            session_count: 1,
          },
        ],
        workspace: '/home/me/project',
      });
    if (url.startsWith('/api/sessions?')) return jsonResponse({ sessions: [summaryFixture] });
    if (url === '/api/sessions') return jsonResponse({ sessions: [summaryFixture] });
    if (url === '/api/sessions/s_ab12') return jsonResponse(detailFixture);
    if (url === '/api/plugins') return jsonResponse({ plugins: [] });
    if (url === '/api/mcp') return jsonResponse({ servers: [] });
    return jsonResponse({ error: { code: 'not_found', message: `unmocked ${url}` } }, 404);
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

describe('App smoke render', () => {
  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
  });

  it('boots from the documented API and renders the workspace with no console errors', async () => {
    installFetch();
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

    const { unmount } = render(<App />);

    expect(screen.getByLabelText('Loading karucode workspace')).not.toBeNull();

    await waitFor(() => {
      expect(screen.getAllByText('Fix login bug').length).toBeGreaterThan(0);
    });
    expect(await screen.findByText('The login handler is unfinished.')).not.toBeNull();
    expect(screen.getByText('I will read the auth module.')).not.toBeNull();
    expect(screen.getAllByText('read_file').length).toBeGreaterThan(0);
    expect(screen.getByText('fn login() { todo!() }')).not.toBeNull();
    expect(screen.getByText('Run cargo test')).not.toBeNull();
    // Two tablists share the page now: the workspace views and the inspector.
    const inspector = screen.getByRole('complementary', { name: 'Workspace inspector' });
    const workspace = screen.getByRole('main', { name: 'Workspace' });
    expect(within(workspace).getByRole('tab', { name: 'Conversation' })).not.toBeNull();
    // The Changes tab may carry a changed-file count pill, hence the prefix match.
    expect(within(workspace).getByRole('tab', { name: /^Changes/ })).not.toBeNull();
    expect(within(workspace).queryByRole('tab', { name: /terminal/i })).toBeNull();
    expect(within(inspector).getByRole('tab', { name: 'Files' })).not.toBeNull();
    expect(within(inspector).getByRole('tab', { name: 'Changes' })).not.toBeNull();
    expect(within(inspector).getByRole('tab', { name: /^Session/ })).not.toBeNull();
    expect(within(inspector).getByRole('tab', { name: /^Agents/ })).not.toBeNull();
    expect(screen.getByLabelText('Message karucode')).not.toBeNull();

    await act(async () => {
      await Promise.resolve();
    });

    expect(errorSpy).not.toHaveBeenCalled();
    expect(warnSpy).not.toHaveBeenCalled();
    unmount();
    errorSpy.mockRestore();
    warnSpy.mockRestore();
  });

  it('shows the token entry screen on 401', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        jsonResponse({ error: { code: 'unauthorized', message: 'token required' } }, 401),
      ),
    );
    render(<App />);
    expect(await screen.findByText('Authentication required')).not.toBeNull();
    expect(screen.getByLabelText('Access token')).not.toBeNull();
  });
});
describe('App crash containment', () => {
  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    resetAppState();
    vi.unstubAllGlobals();
    vi.doUnmock('../components/Inspector');
    vi.resetModules();
  });

  it('keeps the rails, the transcript and the composer when the inspector throws', async () => {
    // A panel crash used to take the whole document down: there were two
    // boundaries in the app (the conversation and a boot fallback), so anything
    // that threw inside the inspector, the rails or an overlay unmounted the
    // entire tree and left a blank page. This is the assertion that the shell
    // survives one panel dying - it is not a rendering test of the inspector.
    vi.resetModules();
    vi.doMock('../components/Inspector', () => ({
      Inspector: () => {
        throw new Error('plugin panel payload was not JSON');
      },
    }));
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const { App: MockedApp } = await import('../app');
    installFetch();

    render(<MockedApp />);

    await waitFor(() => {
      expect(screen.getAllByText('Fix login bug').length).toBeGreaterThan(0);
    });
    // The transcript comes from the session detail read, which lands after the
    // list; wait for it so the assertions below are about containment rather than
    // about a request that had not finished.
    await waitFor(() => {
      expect(screen.getByText('The login handler is unfinished.')).not.toBeNull();
    });

    // The crash card names the panel...
    expect(screen.getByText('The inspector could not be rendered.')).not.toBeNull();
    expect(screen.getByText('plugin panel payload was not JSON')).not.toBeNull();
    // ...and everything else is still on screen and still usable.
    expect(screen.getByLabelText('Message karucode')).not.toBeNull();
    expect(screen.getByRole('main', { name: 'Workspace' })).not.toBeNull();
    expect(screen.getByLabelText('Find a session')).not.toBeNull();
    errorSpy.mockRestore();
  });
});

/** Preact flushes effects in an animation frame, which jsdom defers; let it run. */
async function flushEffects(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 32));
  });
}

describe('Shell shortcuts', () => {
  beforeEach(() => {
    resetAppState();
    store.themeStudioOpen.value = false;
  });

  afterEach(() => {
    resetAppState();
    store.themeStudioOpen.value = false;
    vi.unstubAllGlobals();
  });

  it('opens and closes the Theme Studio with Ctrl/Cmd+Shift+T', async () => {
    installFetch();
    render(<App />);
    await waitFor(() => expect(store.boot.value).toBe('ready'));
    await flushEffects();

    fireEvent.keyDown(window, { key: 'T', ctrlKey: true, shiftKey: true });
    expect(store.themeStudioOpen.value).toBe(true);

    fireEvent.keyDown(window, { key: 'T', ctrlKey: true, shiftKey: true });
    expect(store.themeStudioOpen.value).toBe(false);

    fireEvent.keyDown(window, { key: 'T', metaKey: true, shiftKey: true });
    expect(store.themeStudioOpen.value).toBe(true);

    fireEvent.keyDown(window, { key: 'Escape' });
    expect(store.themeStudioOpen.value).toBe(false);
  });

  it('focuses the session search on "/" but not while typing in the composer', async () => {
    installFetch();
    render(<App />);
    await waitFor(() => expect(store.boot.value).toBe('ready'));
    await flushEffects();
    const search = screen.getByLabelText('Find a session') as HTMLInputElement;

    fireEvent.keyDown(window, { key: '/' });
    await waitFor(() => expect(document.activeElement).toBe(search));

    const composer = screen.getByLabelText('Message karucode');
    composer.focus();
    const event = new KeyboardEvent('keydown', { key: '/', bubbles: true, cancelable: true });
    composer.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(composer);
  });

  it('creates a session with Ctrl/Cmd+N, but not while typing in the composer', async () => {
    const fetchMock = installFetch();
    render(<App />);
    await waitFor(() => expect(store.boot.value).toBe('ready'));
    await flushEffects();

    const sessionPosts = (): number =>
      fetchMock.mock.calls.filter(([input, init]) => {
        const url = typeof input === 'string' ? input : String(input);
        return url === '/api/sessions' && (init?.method ?? 'GET').toUpperCase() === 'POST';
      }).length;

    // Typing in the composer: Ctrl+N is a text-editing key, so it must be left
    // alone — no session, and the browser default is not swallowed.
    const composer = screen.getByLabelText('Message karucode');
    composer.focus();
    const typing = new KeyboardEvent('keydown', {
      key: 'n',
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    composer.dispatchEvent(typing);
    expect(typing.defaultPrevented).toBe(false);
    expect(sessionPosts()).toBe(0);

    // Anywhere else it still creates one.
    fireEvent.keyDown(window, { key: 'n', ctrlKey: true });
    await waitFor(() => expect(sessionPosts()).toBe(1));
  });
});
