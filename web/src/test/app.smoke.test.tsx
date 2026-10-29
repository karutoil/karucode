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
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
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
    expect(within(inspector).getByRole('tab', { name: /^Commands/ })).not.toBeNull();
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
});
