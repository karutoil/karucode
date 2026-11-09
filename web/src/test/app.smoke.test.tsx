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
import { controlPlaneOpen } from '../state/controlPlane';

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

function installFetch(config: typeof configFixture = configFixture) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
    void init;
    if (url.startsWith('/api/events')) return openEventStream();
    if (url === '/api/health') return jsonResponse(healthFixture);
    if (url === '/api/config') return jsonResponse(config);
    if (url.includes('/ui/actions/')) {
      return jsonResponse({ ok: true, message: 'Focused', refresh: [], open_url: null });
    }
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
    if (url === '/api/onboarding') {
      // An established install: the smoke test is about the workspace, not the
      // first-run wizard.
      return jsonResponse({
        needed: false,
        completed: true,
        provider_configured: true,
        has_sessions: true,
        project_count: 1,
        has_credential: false,
        access: {
          enabled: false,
          password_set: false,
          passkeys: [],
          sessions: [],
          public_url: null,
          session_days: 30,
          tunnel: 'none',
          trusted_proxies: [],
          trusts_forwarded_headers: false,
          cloudflare_ranges: false,
          token_required: false,
          settings_locked: false,
        },
        workspace: '/home/me/project',
        version: '0.1.0',
      });
    }
    if (url === '/api/auth/status') {
      return jsonResponse({
        enabled: false,
        sign_in_required: false,
        password_set: false,
        passkey_count: 0,
        needs_setup: true,
        signed_in: false,
        signed_in_with: null,
        token_required: false,
        session_days: 30,
        public_url: null,
        tunnel: 'none',
        trusted_proxies: [],
        cloudflare_proxies: false,
        origin: 'http://127.0.0.1:8787',
        rp_id: '127.0.0.1',
        min_password_len: 10,
        max_password_len: 1024,
        passkey_challenge_ttl_secs: 300,
      });
    }
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
    expect(screen.queryByText('fn login() { todo!() }')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /^read_file src\/auth\.rs\b/i }));
    expect(screen.getByText('fn login() { todo!() }')).not.toBeNull();
    expect(screen.getByText('Run cargo test')).not.toBeNull();
    // Two tablists share the page now: header workspace views and true results.
    const inspector = screen.getByRole('complementary', { name: 'Workspace inspector' });
    const workspace = screen.getByRole('main', { name: 'Workspace' });
    const workspaceViews = within(workspace).getByRole('tablist', { name: 'Workspace views' });
    expect(workspaceViews.closest('header.main-head')).not.toBeNull();
    expect(workspace.querySelector('.workspace-tabbar')).toBeNull();
    expect(within(workspace).getByRole('tab', { name: 'Conversation' })).not.toBeNull();
    // The source-control tab may carry a changed-file count pill, hence the
    // prefix match.
    expect(within(workspace).getByRole('tab', { name: /^Source control/ })).not.toBeNull();
    expect(within(workspace).queryByRole('tab', { name: /terminal/i })).toBeNull();
    expect(within(inspector).getByRole('tab', { name: 'Files' })).not.toBeNull();
    expect(within(inspector).getByRole('tab', { name: 'Git' })).not.toBeNull();
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

  it('opens the first-run wizard instead of the workspace on a fresh install', async () => {
    // The regression this exists for: a boot that fetched `/api/onboarding` but
    // ignored `needed` left a brand-new install staring at an empty workspace
    // with no provider and no way to set one.
    const fetchMock = installFetch();
    const base = fetchMock.getMockImplementation()!;
    vi.stubGlobal(
      'fetch',
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
        if (url === '/api/onboarding') {
          const response = await base(input, init);
          const body = await response.json();
          return jsonResponse({ ...body, needed: true, completed: false });
        }
        return base(input, init);
      }),
    );
    render(<App />);
    expect(await screen.findByRole('heading', { name: 'Welcome to karucode' })).not.toBeNull();
    expect(screen.queryByLabelText('Message karucode')).toBeNull();
  });

  it('shows the sign-in screen on 401', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        jsonResponse({ error: { code: 'unauthorized', message: 'token required' } }, 401),
      ),
    );
    render(<App />);
    expect(await screen.findByRole('heading', { name: 'Sign in' })).not.toBeNull();
    // With no /api/auth/status answer the screen cannot know which method the
    // server offers, so it shows the bearer token form - the one method every
    // version supports.
    expect(screen.getByLabelText('Access token')).not.toBeNull();
  });
  it('does not retain an ordinary Workbench beneath the Control Plane', async () => {
    installFetch();
    render(<App />);
    await waitFor(() => expect(store.boot.value).toBe('ready'));

    fireEvent.click(screen.getByRole('button', { name: 'Task actions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'All tasks' }));
    await waitFor(() => expect(controlPlaneOpen.value).toBe(true));
    expect(document.querySelectorAll('#chat-main')).toHaveLength(0);
    expect(document.querySelectorAll('#composer-input')).toHaveLength(0);

    fireEvent.click(document.querySelector('.project-rail-item') as HTMLButtonElement);
    await waitFor(() => expect(controlPlaneOpen.value).toBe(false));
    expect(document.querySelectorAll('#chat-main')).toHaveLength(1);
    expect(document.querySelectorAll('#composer-input')).toHaveLength(1);
  });

  it('runs a contributed shortcut from the workspace, but never while typing or in a dialog', async () => {
    const config = {
      ...configFixture,
      plugin_ui: {
        contributions: [
          {
            plugin: 'kc-focus',
            version: '1.0.0',
            kind: 'keybinding',
            id: 'focus-key',
            hosted_id: 'plugin__kc-focus__focus-key',
            label: 'Focus mode shortcut',
            keys: 'mod+shift+f',
            action: 'focus',
          },
        ],
      },
    } as typeof configFixture;
    const fetchMock = installFetch(config);
    render(<App />);
    await waitFor(() => expect(store.boot.value).toBe('ready'));

    const mac = /mac|iphone|ipad/i.test(`${navigator.platform} ${navigator.userAgent}`);
    const chord = { key: 'f', shiftKey: true, metaKey: mac, ctrlKey: !mac };
    const actionCalls = () =>
      fetchMock.mock.calls.filter(([url]) => String(url).includes('/ui/actions/focus')).length;

    // While the caret is in the composer the chord belongs to the field.
    fireEvent.keyDown(screen.getByLabelText('Message karucode'), chord);
    await act(async () => {
      await Promise.resolve();
    });
    expect(actionCalls()).toBe(0);

    // With a dialog open nothing fires behind it.
    store.paletteOpen.value = true;
    fireEvent.keyDown(window, chord);
    await act(async () => {
      await Promise.resolve();
    });
    expect(actionCalls()).toBe(0);
    store.paletteOpen.value = false;

    // Anywhere else in the workspace it runs the plugin's action.
    fireEvent.keyDown(window, chord);
    await waitFor(() => expect(actionCalls()).toBe(1));
    // The answer comes back as the plugin's own toast.
    await waitFor(() =>
      expect(store.toasts.value.map((toast) => toast.message)).toContain('Focused'),
    );
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

  it('starts a draft with Ctrl/Cmd+N, but not while typing in the composer', async () => {
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
    // alone — no draft, and the browser default is not swallowed.
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
    expect(store.draftSession.value).toBeNull();

    // Anywhere else it starts a conversation - in the client only. The session row
    // is written by the first message, so an abandoned press leaves nothing behind.
    fireEvent.keyDown(window, { key: 'n', ctrlKey: true });
    await waitFor(() => expect(store.draftSession.value).not.toBeNull());
    expect(store.currentSessionId.value).toBeNull();
    await flushEffects();
    expect(sessionPosts()).toBe(0);
  });
});
