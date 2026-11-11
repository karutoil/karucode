/**
 * PluginInstallSection: the browser half of `karucode plugin install`.
 *
 * Every call is mocked at the endpoints boundary (the routes have their own
 * tests); what is pinned here is the interaction: preview → choose → job →
 * done, the rescan summary, the failure surface, and that a completed job
 * fires the one `onInstalled` refresh the settings tab asked for.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/preact';

import type { PluginInstallJob, PluginInstallPreview } from '../api/endpoints';

const mockPreview = vi.fn();
const mockStart = vi.fn();
const mockGet = vi.fn();
const mockCancel = vi.fn();

vi.mock('../api/endpoints', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/endpoints')>();
  return {
    ...actual,
    previewPluginInstall: (...args: unknown[]) => mockPreview(...args),
    startPluginInstall: (...args: unknown[]) => mockStart(...args),
    getPluginInstallJob: (...args: unknown[]) => mockGet(...args),
    cancelPluginInstall: (...args: unknown[]) => mockCancel(...args),
  };
});

import { PluginInstallSection } from './PluginInstallSection';

function previewFixture(): PluginInstallPreview {
  return {
    source: 'local:/tmp/collection',
    kind: 'local',
    resolved: null,
    plugins: [
      {
        name: 'wordy',
        description: 'Counts words',
        permissions: ['files.read'],
        builds: true,
        already_installed: false,
      },
      {
        name: 'pytools',
        description: 'Python demo tools',
        permissions: [],
        builds: false,
        already_installed: false,
      },
    ],
    notes: [
      'warning: /tmp/collection/wordy/plugin.toml declares `settings.trust`, which are operator-only…',
    ],
  };
}

function runningJob(events: PluginInstallJob['events']): PluginInstallJob {
  return {
    id: 'job-1',
    status: 'running',
    events,
    error: null,
    result: null,
    elapsed_ms: 1400,
  };
}

function completedJob(): PluginInstallJob {
  return {
    id: 'job-1',
    status: 'completed',
    events: [
      { kind: 'note', message: 'note: implied build' },
      { kind: 'building', plugin: 'wordy', command: 'cargo build --release in /tmp/x' },
      { kind: 'installed', plugin: 'wordy', path: '/data/plugins/wordy' },
      { kind: 'done', installed: 1 },
    ],
    error: null,
    result: {
      installed: ['wordy'],
      paths: [{ name: 'wordy', path: '/data/plugins/wordy' }],
      rescan: { added: ['wordy'], removed: [], restarted: [], unchanged: [] },
    },
    elapsed_ms: 4200,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

async function typeSource(value: string) {
  const input = (await screen.findByRole('textbox', { name: 'Source' })) as HTMLInputElement;
  fireEvent.input(input, { target: { value } });
}

describe('PluginInstallSection', () => {
  it('previews a source and shows the review surface', async () => {
    mockPreview.mockResolvedValue(previewFixture());
    render(<PluginInstallSection />);

    await typeSource('/tmp/collection');
    fireEvent.click(await screen.findByRole('button', { name: 'Preview' }));

    expect(await screen.findByText(/2 plugins/)).toBeTruthy();
    // The review surface: names, permissions, build flags, notes.
    expect(screen.getByText('Counts words')).toBeTruthy();
    expect(screen.getByText(/asks: files\.read/)).toBeTruthy();
    expect(screen.getByText(/operator-only/)).toBeTruthy();
    expect(screen.getByText('builds · asks: files.read')).toBeTruthy();
    // Nothing was started.
    expect(mockStart).not.toHaveBeenCalled();
  });

  it(
    'installs the chosen plugins and polls to completion',
    { timeout: 12000 },
    async () => {
    const onInstalled = vi.fn();
    mockPreview.mockResolvedValue(previewFixture());
    mockStart.mockResolvedValue({ id: 'job-1', status: 'running', poll: '/api/plugins/install/job-1' });
    mockGet
      .mockResolvedValueOnce(runningJob([{ kind: 'note', message: 'note: implied build' }]))
      .mockResolvedValueOnce(completedJob());
    render(<PluginInstallSection onInstalled={onInstalled} />);

    await typeSource('/tmp/collection');
    fireEvent.click(await screen.findByRole('button', { name: 'Preview' }));
    // Choose wordy only.
    fireEvent.click(await screen.findByRole('checkbox', { name: /wordy/ }));
    fireEvent.click(await screen.findByRole('button', { name: /Install wordy/ }));

    // The request carries the selection.
    await waitFor(() =>
      expect(mockStart).toHaveBeenCalledWith(
        expect.objectContaining({ source: '/tmp/collection', names: ['wordy'] }),
        expect.anything(),
      ),
    );

    // The poll runs on a real 700ms timeout; the mocks resolve immediately,
    // so two ticks land well inside the waitFor window.

    // Progress lines and the result summary. Two poll ticks apart, so the
    // findBy window must cover them.
    const building = await screen.findAllByText(
      (_, el) => (el?.textContent ?? '').includes('Building wordy') && el?.children.length === 0,
      {},
      { timeout: 5000 },
    );
    expect(building.length).toBeGreaterThan(0);
    expect(
      screen.getAllByText(
        (_, el) => (el?.textContent ?? '').includes('Installed wordy') && el?.children.length === 0,
      ).length,
    ).toBeGreaterThan(0);
    expect(
      screen.getAllByText(
        (_, el) => (el?.textContent ?? '').includes('Added: wordy') && el?.children.length === 0,
      ).length,
    ).toBeGreaterThan(0);
    // Exactly one refresh when the job lands.
    expect(onInstalled).toHaveBeenCalledTimes(1);
    // Polling stopped: no further get calls.
    const calls = mockGet.mock.calls.length;
    await new Promise((resolve) => setTimeout(resolve, 1500));
    expect(mockGet.mock.calls.length).toBe(calls);
    },
  );

  it(
    'shows the job failure with its error text and keeps the log',
    { timeout: 10000 },
    async () => {
    mockPreview.mockResolvedValue(previewFixture());
    mockStart.mockResolvedValue({ id: 'job-2', status: 'running', poll: '/x' });
    const failed: PluginInstallJob = {
      id: 'job-2',
      status: 'failed',
      events: [{ kind: 'downloading', url: 'https://codeload.github.com/o/r/tar.gz/HEAD' }],
      error: 'build of `wordy` failed with status exit code: 101: error[E0308]…',
      result: null,
      elapsed_ms: 300,
    };
    mockGet.mockResolvedValue(failed);
    render(<PluginInstallSection />);

    await typeSource('owner/repo');
    fireEvent.click(await screen.findByRole('button', { name: 'Preview' }));
    fireEvent.click(await screen.findByRole('button', { name: /Install everything/ }));
    await waitFor(() => expect(mockStart).toHaveBeenCalled());

    const failures = await screen.findAllByText(
      (_, el) => (el?.textContent ?? '').includes('exit code: 101') && el?.children.length === 0,
      {},
      { timeout: 5000 },
    );
    expect(failures.length).toBeGreaterThan(0);
    // The progress it did make is still visible.
    expect(
      screen.getAllByText(
        (_, el) => (el?.textContent ?? '').includes('Downloading https') && el?.children.length === 0,
      ).length,
    ).toBeGreaterThan(0);
    },
  );

  it('refuses to start with an empty source and surfaces request errors', async () => {
    render(<PluginInstallSection />);
    fireEvent.click(await screen.findByRole('button', { name: 'Preview' }));
    expect(await screen.findByText(/Give a source/)).toBeTruthy();

    mockPreview.mockRejectedValue(new Error('no such repository'));
    await typeSource('owner/nope');
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    expect(await screen.findByText('no such repository')).toBeTruthy();
    expect(mockStart).not.toHaveBeenCalled();
  });

  it('sends the admin token as a header once one is typed', async () => {
    mockPreview
      .mockRejectedValueOnce(
        Object.assign(new Error('previewing a plugin install requires the admin credential'), {
          status: 403,
        }),
      )
      .mockResolvedValue(previewFixture());
    render(<PluginInstallSection />);

    await typeSource('/tmp/collection');
    fireEvent.click(await screen.findByRole('button', { name: 'Preview' }));
    // The 403 surfaces, with the admin-token field next to it.
    const tokenInput = (await screen.findByPlaceholderText(
      'X-Karucode-Admin-Token',
    )) as HTMLInputElement;
    fireEvent.input(tokenInput, { target: { value: 'sekret-token' } });
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));

    await waitFor(() =>
      expect(mockPreview).toHaveBeenLastCalledWith(
        expect.anything(),
        expect.objectContaining({ headers: { 'X-Karucode-Admin-Token': 'sekret-token' } }),
      ),
    );
    expect(await screen.findByText(/2 plugins/)).toBeTruthy();
  });

  it('exposes a stop button that cancels the running job', async () => {
    mockPreview.mockResolvedValue(previewFixture());
    mockStart.mockResolvedValue({ id: 'job-3', status: 'running', poll: '/x' });
    mockGet.mockResolvedValue(runningJob([]));
    mockCancel.mockResolvedValue({ id: 'job-3', status: 'cancelling' });
    render(<PluginInstallSection />);

    await typeSource('/tmp/collection');
    fireEvent.click(await screen.findByRole('button', { name: 'Preview' }));
    // Pick one plugin so the "everything?" guard is not involved.
    fireEvent.click(await screen.findByRole('checkbox', { name: /wordy/ }));
    fireEvent.click(await screen.findByRole('button', { name: /Install wordy/ }));
    // The first poll tick hands the component a running job, which is what
    // makes the Stop button render.
    await waitFor(() => expect(mockGet).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByRole('button', { name: 'Stop' })).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
    expect(mockCancel).toHaveBeenCalledWith('job-3', expect.anything());
  });
});
