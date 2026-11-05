import { afterEach, describe, expect, it, vi } from 'vitest';
import { createArtifact } from '../api/endpoints';
import { publishArtifact, resetArtifactCache } from './artifactPreview';

vi.mock('../api/endpoints', () => ({ createArtifact: vi.fn() }));

const MASTER = 'master-secret-that-must-never-enter-an-artifact-url';
const SCOPED_URL = '/api/artifacts/abc123?capability=one-artifact-only';

afterEach(() => {
  resetArtifactCache();
  vi.clearAllMocks();
  window.localStorage.clear();
});

describe('artifact preview URLs', () => {
  it('uses the server-issued per-artifact capability URL, never the master bearer', async () => {
    window.localStorage.setItem('karucode.auth_token', MASTER);
    vi.mocked(createArtifact).mockResolvedValue({
      id: 'abc123',
      url: SCOPED_URL,
      bytes: 100,
      theme: 'dark',
    });

    await expect(publishArtifact('<p>hostile</p>', 'dark')).resolves.toBe(SCOPED_URL);
    expect(SCOPED_URL).not.toContain(MASTER);
    expect(SCOPED_URL).not.toContain('token=');
    expect(createArtifact).toHaveBeenCalledWith({ html: '<p>hostile</p>', theme: 'dark' });
  });

  it('caches the whole scoped URL and coalesces duplicate publishes', async () => {
    vi.mocked(createArtifact).mockResolvedValue({
      id: 'abc123',
      url: SCOPED_URL,
      bytes: 100,
      theme: 'light',
    });
    const first = publishArtifact('<p>same</p>', 'light');
    const second = publishArtifact('<p>same</p>', 'light');
    await expect(Promise.all([first, second])).resolves.toEqual([SCOPED_URL, SCOPED_URL]);
    await expect(publishArtifact('<p>same</p>', 'light')).resolves.toBe(SCOPED_URL);
    expect(createArtifact).toHaveBeenCalledTimes(1);
  });
});
