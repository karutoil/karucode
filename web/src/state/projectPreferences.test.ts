import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '../api/types';
import * as api from '../api/endpoints';
import * as actions from './actions';
import * as store from './store';
import { jsonResponse, resetAppState } from '../test/fixtures';

function project(width?: number): Project {
  return {
    id: 'p_one',
    name: 'One',
    path: '/tmp/one',
    created_at: 1,
    updated_at: 2,
    session_count: 0,
    ...(width === undefined ? {} : { result_panel_width: width }),
  };
}

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe('project result-width persistence', () => {
  beforeEach(() => {
    resetAppState();
    store.projects.value = [project(400)];
    store.currentProjectId.value = 'p_one';
  });

  afterEach(() => {
    resetAppState();
    store.projects.value = [];
    store.currentProjectId.value = null;
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('uses the project preferences PATCH and merges the saved project record', async () => {
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body)) as { result_panel_width: number };
      return jsonResponse({ project: { ...project(), result_panel_width: body.result_panel_width } });
    });
    vi.stubGlobal('fetch', fetchMock);

    await expect(actions.saveProjectResultPanelWidth('p_one', 639.6)).resolves.toBe(true);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe('/api/projects/p_one/preferences');
    expect(init?.method).toBe('PATCH');
    expect(JSON.parse(String(init?.body))).toEqual({ result_panel_width: 640 });
    expect(store.projects.value[0]?.result_panel_width).toBe(640);
  });

  it('keeps the last server value and reports a sticky failure without throwing', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => jsonResponse({ error: { code: 'network', message: 'offline' } }, 503)));

    await expect(actions.saveProjectResultPanelWidth('p_one', 620)).resolves.toBe(false);

    expect(store.projects.value[0]?.result_panel_width).toBe(400);
    expect(store.toasts.value.at(-1)?.message).toContain('kept for this window');
    expect(store.toasts.value.at(-1)?.message).toContain('offline');
  });

  it('does not let a slow older write overwrite a newer selection', async () => {
    const first = deferred<Project>();
    const second = deferred<Project>();
    const update = vi.spyOn(api, 'updateProjectResultPanelWidth')
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);

    const firstSave = actions.saveProjectResultPanelWidth('p_one', 480);
    const secondSave = actions.saveProjectResultPanelWidth('p_one', 640);
    second.resolve(project(640));
    await secondSave;
    first.resolve(project(480));
    await firstSave;

    expect(update).toHaveBeenCalledTimes(2);
    expect(store.projects.value[0]?.result_panel_width).toBe(640);
  });

  it('treats an unavailable project as a non-persistent local fallback', async () => {
    const update = vi.spyOn(api, 'updateProjectResultPanelWidth');
    await expect(actions.saveProjectResultPanelWidth('p_gone', 620)).resolves.toBe(false);
    expect(update).not.toHaveBeenCalled();
    expect(store.toasts.value).toEqual([]);
  });
});
