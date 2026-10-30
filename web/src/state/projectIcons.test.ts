import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as store from './store';
import { loadProjectIcon, resetProjectIcons, syncProjectIcons } from './projectIcons';

/**
 * The icon loader owns object URLs, so the interesting behaviour is: fetch once
 * per project, remember "no icon" instead of retrying it forever, retry a
 * transport failure later, and never leak a URL.
 */

const created: string[] = [];
const revoked: string[] = [];
let urlSequence = 0;

function installObjectUrls(): void {
  Object.defineProperty(URL, 'createObjectURL', {
    configurable: true,
    value: vi.fn(() => {
      urlSequence += 1;
      const url = `blob:icon-${urlSequence}`;
      created.push(url);
      return url;
    }),
  });
  Object.defineProperty(URL, 'revokeObjectURL', {
    configurable: true,
    value: vi.fn((url: string) => {
      revoked.push(url);
    }),
  });
}

function okBlob(): Response {
  return new Response(new Blob([new Uint8Array([0, 0, 1, 0])], { type: 'image/x-icon' }), {
    status: 200,
    headers: { 'content-type': 'image/x-icon' },
  });
}

function noContent(): Response {
  return new Response(null, { status: 204 });
}

function notFound(): Response {
  return new Response(JSON.stringify({ error: { code: 'not_found', message: 'no icon' } }), {
    status: 404,
    headers: { 'content-type': 'application/json' },
  });
}

function installFetch(response: (url: string) => Response | Promise<Response>) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
    return response(url);
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

beforeEach(() => {
  created.length = 0;
  revoked.length = 0;
  urlSequence = 0;
  installObjectUrls();
  resetProjectIcons();
});

afterEach(() => {
  resetProjectIcons();
  vi.unstubAllGlobals();
});

describe('project icons', () => {
  it('fetches a project icon once and publishes an object URL', async () => {
    const fetchMock = installFetch(() => okBlob());

    await expect(loadProjectIcon('p_one')).resolves.toBe('blob:icon-1');
    expect(store.projectIcons.value).toEqual({ p_one: 'blob:icon-1' });

    // A second call answers from the cache: one request, one URL.
    await expect(loadProjectIcon('p_one')).resolves.toBe('blob:icon-1');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(created).toHaveLength(1);
  });

  it('asks the documented endpoint with the size cap', async () => {
    const fetchMock = installFetch(() => okBlob());

    await loadProjectIcon('p_one');
    const [call] = fetchMock.mock.calls;
    expect(call?.[0]).toBe('/api/projects/p_one/favicon?max_bytes=524288');
  });

  it('treats a 204 as "this project has no icon" and asks only once', async () => {
    // 204 is the server's quiet answer for the common case, so the browser
    // console stays free of `Failed to load resource` noise.
    const fetchMock = installFetch(() => noContent());

    await expect(loadProjectIcon('p_one')).resolves.toBeNull();
    expect(store.projectIcons.value).toEqual({});

    await expect(loadProjectIcon('p_one')).resolves.toBeNull();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(created).toHaveLength(0);
  });

  it('still treats a 404 as "no icon" for an older server', async () => {
    const fetchMock = installFetch(() => notFound());
    await expect(loadProjectIcon('p_one')).resolves.toBeNull();
    await expect(loadProjectIcon('p_one')).resolves.toBeNull();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('retries a transport failure later rather than caching it forever', async () => {
    let calls = 0;
    installFetch(() => {
      calls += 1;
      if (calls === 1) throw new TypeError('network down');
      return okBlob();
    });

    await expect(loadProjectIcon('p_one')).resolves.toBeNull();
    // The retry window has not elapsed, so the failure is remembered for now.
    await expect(loadProjectIcon('p_one')).resolves.toBeNull();
    expect(calls).toBe(1);

    // Forcing the retry (what happens once the window elapses and the list is
    // refetched) picks the icon up.
    await expect(loadProjectIcon('p_one', true)).resolves.toBe('blob:icon-1');
    expect(calls).toBe(2);
  });

  it('coalesces concurrent loads into one request', async () => {
    // A response that only lands when the test says so, which is the window in
    // which a second caller has to find the in-flight entry.
    let release = (): void => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const fetchMock = vi.fn(async () => {
      await gate;
      return okBlob();
    });
    vi.stubGlobal('fetch', fetchMock);

    const first = loadProjectIcon('p_one');
    const second = loadProjectIcon('p_one');
    release();
    await expect(Promise.all([first, second])).resolves.toEqual(['blob:icon-1', 'blob:icon-1']);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('revokes the object URL of a project that disappears', async () => {
    installFetch(() => okBlob());
    syncProjectIcons([{ id: 'p_one' }, { id: 'p_two' }]);
    await vi.waitFor(() => expect(store.projectIcons.value).toHaveProperty('p_two'));

    syncProjectIcons([{ id: 'p_one' }]);
    // Both URLs were created; only the dropped project's is revoked.
    expect(revoked).toHaveLength(1);
    expect(Object.keys(store.projectIcons.value)).toEqual(['p_one']);
  });

  it('keeps a project icon when the list is refreshed with the same project', async () => {
    const fetchMock = installFetch(() => okBlob());
    syncProjectIcons([{ id: 'p_one' }]);
    await vi.waitFor(() => expect(store.projectIcons.value.p_one).toBeDefined());

    syncProjectIcons([{ id: 'p_one' }]);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(revoked).toHaveLength(0);
  });

  it('forgets every icon when the last project is deleted', async () => {
    installFetch(() => okBlob());
    syncProjectIcons([{ id: 'p_one' }, { id: 'p_two' }]);
    await vi.waitFor(() => expect(Object.keys(store.projectIcons.value)).toHaveLength(2));

    // `deleteProjectById` passes the remaining projects, which is how a deleted
    // project's icon is dropped and its object URL revoked.
    syncProjectIcons([]);
    expect(store.projectIcons.value).toEqual({});
    expect(revoked).toHaveLength(2);
  });

  it('resolves null when the browser cannot create object URLs', async () => {
    Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: undefined });
    installFetch(() => okBlob());

    await expect(loadProjectIcon('p_one')).resolves.toBeNull();
    expect(store.projectIcons.value).toEqual({});
  });
});
