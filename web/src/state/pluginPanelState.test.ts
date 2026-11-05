import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { errorResponse, jsonResponse, resetAppState } from '../test/fixtures';
import * as actions from './actions';
import * as store from './store';

const panel: store.PluginPanelTab = {
  id: 'status',
  hosted_id: 'plugin__demo__status',
  title: 'Demo status',
  icon: null,
  source: 'tool',
  refresh_ms: 0,
  plugin: 'demo',
  version: '1.0.0',
};

const realFetch = globalThis.fetch;

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function stubPanels(): {
  reads: ReturnType<typeof deferred<Response>>[];
  fetchMock: ReturnType<typeof vi.fn>;
} {
  const reads: ReturnType<typeof deferred<Response>>[] = [];
  const impl = vi.fn(async (input: RequestInfo | URL) => {
    void input;
    const read = deferred<Response>();
    reads.push(read);
    return read.promise;
  });
  globalThis.fetch = impl as unknown as typeof fetch;
  return { reads, fetchMock: impl };
}

function readAt(
  reads: ReturnType<typeof deferred<Response>>[],
  index: number,
): ReturnType<typeof deferred<Response>> {
  const read = reads[index];
  if (!read) throw new Error(`missing panel read ${index}`);
  return read;
}

function panelPayload(context: string): Response {
  return jsonResponse({ title: `Status ${context}`, data: { context }, fetched_ms: 10 });
}

beforeEach(() => {
  resetAppState();
  store.currentProjectId.value = 'p_one';
  store.currentSessionId.value = 's_one';
});

afterEach(() => {
  globalThis.fetch = realFetch;
  resetAppState();
  vi.restoreAllMocks();
});

describe('plugin panel context isolation', () => {
  it.each([
    { label: 'project and session', projectId: 'p_two', sessionId: 's_two' },
    { label: 'session within one project', projectId: 'p_one', sessionId: 's_two' },
  ])('keys in-flight reads and results when the $label changes', async ({ projectId, sessionId }) => {
    const { reads, fetchMock } = stubPanels();
    const first = actions.refreshPluginPanel(panel);
    await vi.waitFor(() => expect(reads.length).toBe(1));

    store.currentProjectId.value = projectId;
    store.currentSessionId.value = sessionId;
    const second = actions.refreshPluginPanel(panel);
    await vi.waitFor(() => expect(reads.length).toBe(2));
    expect(second).not.toBe(first);
    const urls = fetchMock.mock.calls.map(([input]) => String(input));
    expect(urls[0]).toContain('project=p_one');
    expect(urls[0]).toContain('session=s_one');
    expect(urls[1]).toContain(`project=${projectId}`);
    expect(urls[1]).toContain(`session=${sessionId}`);
    expect(store.pluginPanelData.value[panel.hosted_id]?.loading).toBe(true);

    readAt(reads, 1).resolve(panelPayload('two'));
    await second;
    expect(store.pluginPanelData.value[panel.hosted_id]).toMatchObject({
      loading: false,
      data: { context: 'two' },
      error: null,
    });

    // Even an old failure must publish neither A's error nor its loading cleanup
    // into B's visible panel.
    readAt(reads, 0).resolve(errorResponse('plugin', 'old context failed', 502));
    await first;
    expect(store.pluginPanelData.value[panel.hosted_id]).toMatchObject({
      loading: false,
      data: { context: 'two' },
      error: null,
    });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('reset invalidates old responses and lets the same context start a fresh request', async () => {
    const { reads, fetchMock } = stubPanels();
    const first = actions.refreshPluginPanel(panel);
    await vi.waitFor(() => expect(reads.length).toBe(1));

    actions.resetPluginPanelData();
    const replacement = actions.refreshPluginPanel(panel);
    await vi.waitFor(() => expect(reads.length).toBe(2));
    expect(replacement).not.toBe(first);
    expect(store.pluginPanelData.value[panel.hosted_id]?.loading).toBe(true);

    readAt(reads, 0).resolve(panelPayload('stale before reset'));
    await first;
    expect(store.pluginPanelData.value[panel.hosted_id]?.loading).toBe(true);
    expect(store.pluginPanelData.value[panel.hosted_id]?.data).toBeNull();

    readAt(reads, 1).resolve(panelPayload('fresh after reset'));
    await replacement;
    expect(store.pluginPanelData.value[panel.hosted_id]).toMatchObject({
      loading: false,
      data: { context: 'fresh after reset' },
      error: null,
    });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });
});
