import { afterEach, describe, expect, it, vi } from 'vitest';
import { createArtifact } from '../api/endpoints';
import { invalidateArtifact, publishArtifact, resetArtifactCache } from './artifactPreview';

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

  it('posts the source again after explicit invalidation instead of reusing a dead URL', async () => {
    const freshUrl = '/api/artifacts/new456?capability=new-scoped-read';
    vi.mocked(createArtifact)
      .mockResolvedValueOnce({ id: 'abc123', url: SCOPED_URL, bytes: 100, theme: 'dark' })
      .mockResolvedValueOnce({ id: 'new456', url: freshUrl, bytes: 100, theme: 'dark' });

    await expect(publishArtifact('<p>restart</p>', 'dark')).resolves.toBe(SCOPED_URL);
    // A normal second read is the cache hit that used to keep the dead 404 URL.
    await expect(publishArtifact('<p>restart</p>', 'dark')).resolves.toBe(SCOPED_URL);
    expect(createArtifact).toHaveBeenCalledTimes(1);

    invalidateArtifact('<p>restart</p>', 'dark');
    await expect(publishArtifact('<p>restart</p>', 'dark')).resolves.toBe(freshUrl);
    expect(createArtifact).toHaveBeenCalledTimes(2);
    expect(createArtifact).toHaveBeenLastCalledWith({ html: '<p>restart</p>', theme: 'dark' });
  });

  it('does not let an invalidated in-flight POST refill the cache', async () => {
    let answerFirst!: (value: { id: string; url: string; bytes: number; theme: 'dark' }) => void;
    vi.mocked(createArtifact)
      .mockImplementationOnce(
        (() => new Promise((resolve) => { answerFirst = resolve; })) as typeof createArtifact,
      )
      .mockResolvedValueOnce({
        id: 'new456',
        url: '/api/artifacts/new456?capability=new',
        bytes: 100,
        theme: 'dark',
      });
    const old = publishArtifact('<p>overlap</p>', 'dark');
    invalidateArtifact('<p>overlap</p>', 'dark');
    await expect(publishArtifact('<p>overlap</p>', 'dark')).resolves.toContain('/new456');
    answerFirst({ id: 'old', url: '/api/artifacts/old?capability=dead', bytes: 100, theme: 'dark' });
    await expect(old).resolves.toContain('/old');

    await expect(publishArtifact('<p>overlap</p>', 'dark')).resolves.toContain('/new456');
    expect(createArtifact).toHaveBeenCalledTimes(2);
  });
});
