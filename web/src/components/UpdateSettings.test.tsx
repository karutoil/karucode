/**
 * The Updates tab of the settings sheet.
 *
 * What matters here, and therefore what these tests assert:
 *
 * 1. **The running build is named, not implied.** The version is a commit count,
 *    so "build 107, commit abf0a42" is the whole answer to "what am I running".
 * 2. **The install button is only offered when it can work.** A root-owned binary
 *    (the normal systemd install) cannot be replaced by the service account, and a
 *    button that always fails is worse than the command that does not.
 * 3. **A check that fails says why.** The server records the reason in the status
 *    and answers with an error; the panel must show it instead of looking idle.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import type { UpdateStatus } from '../api/types';
import { UpdateSettings } from './UpdateSettings';
import { configFixture, NOW, resetAppState } from '../test/fixtures';
import * as store from '../state/store';

vi.mock('../api/endpoints', () => ({
  getUpdate: vi.fn(),
  checkForUpdate: vi.fn(),
  applyUpdate: vi.fn(),
  patchConfig: vi.fn(),
  getConfig: vi.fn(),
  getHealth: vi.fn(),
}));

import * as api from '../api/endpoints';

const MANAGEABLE: UpdateStatus['install'] = {
  kind: 'managed',
  path: '/usr/local/bin/karucode',
  writable: true,
  detail: '/usr/local/bin/karucode is writable by this process: an update installs in place',
};

const ROOT_OWNED: UpdateStatus['install'] = {
  kind: 'managed',
  path: '/usr/local/bin/karucode',
  writable: false,
  detail:
    '/usr/local/bin/karucode is not writable by this process: an update is staged, then installed with `sudo karucode update --apply`',
};

const SOURCE_TREE: UpdateStatus['install'] = {
  kind: 'source',
  path: '/home/me/karucode/target/release/karucode',
  writable: false,
  detail:
    'this binary runs from a build tree (/home/me/karucode/target/release/karucode); `packaging/update.sh` rebuilds it from the checkout',
};

function latest(build = 108): NonNullable<UpdateStatus['latest']> {
  return {
    build,
    version: String(build),
    commit: '9c1de77a0b3c',
    commit_date: '2026-05-31T09:12:40+00:00',
    released_at: '2026-05-31T09:20:03Z',
    notes: 'Faster startup, and the update system itself.',
    url: 'https://example.com/releases/tag/v108',
    artifact: {
      url: 'https://example.com/karucode-108-x86_64-unknown-linux-gnu',
      sha256: 'a'.repeat(64),
      size: 18432120,
      tarball: 'https://example.com/karucode-108.tar.gz',
    },
    has_target_for_this_build: true,
  };
}

function status(overrides: Partial<UpdateStatus> = {}): UpdateStatus {
  return {
    current: {
      version: '107',
      build: 107,
      commit: 'abf0a42',
      commit_date: '2026-05-30T18:04:11+00:00',
      dirty: false,
      channel: 'stable',
      target: 'x86_64-unknown-linux-gnu',
    },
    install: MANAGEABLE,
    check: {
      enabled: true,
      manifest_url: 'https://example.com/updates.json',
      interval_secs: 21_600,
      checked_at: NOW,
      error: null,
    },
    latest: latest(),
    update_available: true,
    phase: 'idle',
    staged: null,
    last_applied: null,
    restart: {
      supervised: true,
      apply_command: 'sudo /usr/local/bin/karucode update --apply --data-dir /var/lib/karucode',
    },
    ...overrides,
  };
}

function renderPanel(): void {
  render(<UpdateSettings disabled={false} />);
}

beforeEach(() => {
  resetAppState();
  store.config.value = configFixture;
  vi.mocked(api.getUpdate).mockResolvedValue(status());
  vi.mocked(api.getConfig).mockResolvedValue(configFixture);
  vi.mocked(api.patchConfig).mockImplementation(async (patch) => ({
    ...configFixture,
    updates: { ...configFixture.updates!, ...(patch.updates ?? {}) },
  }));
  vi.mocked(api.getHealth).mockRejectedValue(new Error('not under test'));
});

afterEach(() => {
  vi.clearAllMocks();
});

describe('this build', () => {
  it('names the build number, the commit and where the binary lives', () => {
    store.updateStatus.value = status();
    renderPanel();
    expect(screen.getByText('build 107')).toBeTruthy();
    expect(screen.getByText(/abf0a42/)).toBeTruthy();
    expect(screen.getByText('/usr/local/bin/karucode')).toBeTruthy();
    expect(screen.getByText(/writable by this process/)).toBeTruthy();
  });

  it('says the version is the commit count and shows a dirty tree', () => {
    store.updateStatus.value = status({
      current: { ...status().current, dirty: true },
    });
    renderPanel();
    expect(screen.getByText(/dirty tree/)).toBeTruthy();
  });

  it('does not invent a version when the server has no update route', () => {
    // An older server answers 404 for /api/update; the panel must say so rather
    // than render a blank build number.
    store.updateStatus.value = null;
    vi.mocked(api.getUpdate).mockRejectedValue(new Error('404'));
    renderPanel();
    expect(screen.getByText(/cannot be named/i)).toBeTruthy();
    expect(screen.getByText(/karucode --version/)).toBeTruthy();
  });
});

describe('an available update', () => {
  it('shows the newer build and its notes', () => {
    store.updateStatus.value = status();
    renderPanel();
    expect(screen.getByText('build 108 available')).toBeTruthy();
    expect(screen.getByText(/Faster startup/)).toBeTruthy();
    expect(screen.getByText('Release page')).toBeTruthy();
  });

  it('installs through the API and reports what happened', async () => {
    store.updateStatus.value = status();
    vi.mocked(api.applyUpdate).mockResolvedValue({
      update: status({
        update_available: false,
        latest: latest(),
        last_applied: {
          from: '107',
          to: '108',
          path: '/usr/local/bin/karucode',
          applied_at: NOW,
        },
      }),
      result: {
        installed: true,
        restart_required: true,
        restart_requested: false,
        from: '107',
        to: '108',
        path: '/usr/local/bin/karucode',
        needs_privilege: false,
        command: null,
        message: 'installed; restart the service to run the new build (systemctl restart karucode)',
      },
    });
    renderPanel();
    fireEvent.click(screen.getByRole('button', { name: 'Install build 108' }));
    await waitFor(() => expect(vi.mocked(api.applyUpdate)).toHaveBeenCalledWith({ restart: false }));
    expect(await screen.findByText(/restart the service to run the new build/)).toBeTruthy();
    // The store is the update's source of truth, so it must carry the new status.
    expect(store.updateStatus.value?.last_applied?.to).toBe('108');
  });

  it('sends restart=true only when the checkbox was ticked', async () => {
    store.updateStatus.value = status();
    vi.mocked(api.applyUpdate).mockResolvedValue({
      update: status(),
      result: {
        installed: true,
        restart_required: true,
        restart_requested: true,
        from: '107',
        to: '108',
        path: '/usr/local/bin/karucode',
        needs_privilege: false,
        command: null,
        message: 'installed; the service is restarting onto the new build',
      },
    });
    renderPanel();
    fireEvent.click(screen.getByRole('checkbox', { name: /Restart the service afterwards/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Install build 108' }));
    await waitFor(() => expect(vi.mocked(api.applyUpdate)).toHaveBeenCalledWith({ restart: true }));
  });

  it('offers the privileged command instead of a button that cannot work', () => {
    store.updateStatus.value = status({ install: ROOT_OWNED });
    renderPanel();
    expect(screen.queryByRole('button', { name: 'Install build 108' })).toBeNull();
    expect(screen.getByText(/may not write its own binary/)).toBeTruthy();
    expect(
      screen.getByText('sudo /usr/local/bin/karucode update --apply --data-dir /var/lib/karucode'),
    ).toBeTruthy();
  });

  it('points a build tree at packaging/update.sh', () => {
    store.updateStatus.value = status({ install: SOURCE_TREE });
    renderPanel();
    expect(screen.getByText(/Update it from the checkout/)).toBeTruthy();
    expect(screen.getByText('sudo sh packaging/update.sh')).toBeTruthy();
  });

  it('does not offer to install a release that has no build for this platform', () => {
    const onlyArm = latest(108);
    onlyArm.has_target_for_this_build = false;
    onlyArm.artifact = null;
    store.updateStatus.value = status({ latest: onlyArm });
    renderPanel();
    expect(screen.getByText(/no build for x86_64-unknown-linux-gnu/)).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Install build 108' })).toBeNull();
  });

  it('does not offer to install a build that is already on disk', () => {
    // The install succeeded but the process still runs the old binary: the only
    // thing left is a restart, so the click target must not come back.
    store.updateStatus.value = status({
      last_applied: {
        from: '107',
        to: '108',
        path: '/usr/local/bin/karucode',
        applied_at: NOW,
      },
    });
    renderPanel();
    expect(screen.queryByRole('button', { name: 'Install build 108' })).toBeNull();
    expect(screen.getByText(/Build 108 is installed. Restart the service to run it/)).toBeTruthy();
  });

  it('says it is up to date when the newest build is the running one', () => {
    store.updateStatus.value = status({ update_available: false, latest: latest(107) });
    renderPanel();
    expect(screen.getByText('Build 107 is the newest release.')).toBeTruthy();
  });
});

describe('checking', () => {
  it('runs a check on demand and reports the result', async () => {
    store.updateStatus.value = status({ update_available: false, latest: latest(107) });
    vi.mocked(api.checkForUpdate).mockResolvedValue(status());
    renderPanel();
    fireEvent.click(screen.getByRole('button', { name: 'Check now' }));
    await waitFor(() => expect(vi.mocked(api.checkForUpdate)).toHaveBeenCalled());
    expect(await screen.findByText('Build 108 is available.')).toBeTruthy();
  });

  it('shows the reason a check failed instead of looking idle', async () => {
    store.updateStatus.value = status({
      update_available: false,
      latest: null,
      check: { ...status().check, error: 'could not fetch https://example.com/updates.json: HTTP 404' },
    });
    renderPanel();
    expect(screen.getByText(/HTTP 404/)).toBeTruthy();
  });
});

describe('automatic checks', () => {
  it('saves the master switch through PATCH /api/config', async () => {
    store.updateStatus.value = status();
    renderPanel();
    fireEvent.click(screen.getByRole('switch', { name: 'Check for updates automatically' }));
    await waitFor(() => expect(vi.mocked(api.patchConfig)).toHaveBeenCalledWith({ updates: { check: false } }));
  });

  it('saves the interval in seconds, from minutes', async () => {
    store.updateStatus.value = status();
    renderPanel();
    const field = screen.getByLabelText(/Check every \(minutes\)/) as HTMLInputElement;
    expect(field.value).toBe('360');
    fireEvent.input(field, { target: { value: '15' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() =>
      expect(vi.mocked(api.patchConfig)).toHaveBeenCalledWith({ updates: { check_secs: 900 } }),
    );
  });

  it('never turns auto-apply on without a click, and says what it costs', () => {
    store.updateStatus.value = status();
    renderPanel();
    const toggle = screen.getByRole('switch', {
      name: 'Install automatically when a newer build is found',
    });
    expect(toggle.getAttribute('aria-checked')).toBe('false');
    expect(screen.getByText(/ends every run in flight/)).toBeTruthy();
  });

  it('explains that the manifest URL is read-only without the admin credential', () => {
    store.updateStatus.value = status();
    renderPanel();
    expect(screen.getByText(/Changing it needs the admin credential/)).toBeTruthy();
  });

  it('reports a refused save with the credential it needs', async () => {
    store.updateStatus.value = status();
    const { ApiError } = await import('../api/client');
    vi.mocked(api.patchConfig).mockRejectedValue(
      new ApiError('changing updates.manifest_url requires the admin credential', 403, 'permission_denied'),
    );
    renderPanel();
    fireEvent.click(screen.getByRole('switch', { name: 'Check for updates automatically' }));
    expect(await screen.findByText(/needs the admin credential/)).toBeTruthy();
  });
});
