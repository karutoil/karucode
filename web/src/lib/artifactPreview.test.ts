import { afterEach, describe, expect, it, vi } from 'vitest';
import { createArtifact } from '../api/endpoints';
import {
  artifactCacheStats,
  invalidateArtifact,
  publishArtifact,
  resetArtifactCache,
} from './artifactPreview';

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

describe('the artifact cache budget', () => {
  /** A resolved publish whose URL length is realistic for a scoped capability. */
  function publishLarge(): void {
    vi.mocked(createArtifact).mockImplementation(async ({ html }) => ({
      id: `${html.length}`,
      url: `/api/artifacts/${html.length}?capability=one-artifact-only`,
      bytes: html.length,
      theme: 'dark',
    }));
  }

  it('retains less than eight megabytes of source across twenty 1 MiB mocks', async () => {
    publishLarge();
    // A cache key *is* the source, so each entry costs its own bytes: 128 of
    // these was ~128 MB of strings alive for the tab's whole life.
    const megabyte = 'x'.repeat(1024 * 1024);
    for (let index = 0; index < 20; index += 1) {
      await publishArtifact(`${index}:${megabyte}`, 'dark');
    }

    const stats = artifactCacheStats();
    expect(stats.bytes).toBeLessThanOrEqual(8 * 1024 * 1024);
    expect(stats.entries).toBeGreaterThan(0);
    expect(stats.entries).toBeLessThan(20);
  });

  it('drops a source generation when its entry is evicted', async () => {
    publishLarge();
    const megabyte = 'y'.repeat(1024 * 1024);
    const kept = `kept:${megabyte}`;
    // Two invalidations of one source make a generation that used to live
    // forever, whether or not the source was still cached.
    invalidateArtifact(kept, 'dark');
    invalidateArtifact(kept, 'dark');
    expect(artifactCacheStats().generations).toBe(1);

    await publishArtifact(kept, 'dark');
    for (let index = 0; index < 12; index += 1) {
      await publishArtifact(`later-${index}:${megabyte}`, 'dark');
    }

    // Its entry is gone (evicted oldest-first) and the generation went with it.
    expect(artifactCacheStats().generations).toBe(0);
  });

  it('keeps an invalidated source from refilling the cache while its POST is running', async () => {
    let answer!: (value: { id: string; url: string; bytes: number; theme: 'dark' }) => void;
    vi.mocked(createArtifact)
      .mockImplementationOnce(
        (() => new Promise((resolve) => { answer = resolve; })) as typeof createArtifact,
      )
      .mockResolvedValue({
        id: 'fresh',
        url: '/api/artifacts/fresh?capability=new',
        bytes: 1,
        theme: 'dark',
      });
    const megabyte = 'z'.repeat(1024 * 1024);
    const source = `flight:${megabyte}`;
    const pending = publishArtifact(source, 'dark');
    invalidateArtifact(source, 'dark');
    // More invalidation than the generation record may keep, so the oldest
    // records are pruned - except the one still guarding a running POST.
    const padding = 'p'.repeat(64 * 1024);
    for (let index = 0; index < 200; index += 1) {
      invalidateArtifact(`${index}:${padding}`, 'dark');
    }
    answer({ id: 'old', url: '/api/artifacts/old?capability=dead', bytes: 1, theme: 'dark' });
    await pending;

    // Had that generation been pruned, the stale URL would be cached now and
    // this would hand it back without a second POST.
    await expect(publishArtifact(source, 'dark')).resolves.toContain('/fresh');
    expect(createArtifact).toHaveBeenCalledTimes(2);
  });
});
