import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, apiRequest } from './client';

/**
 * A fetch that never settles on its own: it only rejects when the signal it was
 * handed aborts, exactly like a real hung request.
 */
function hangingFetch(): typeof fetch {
  return vi.fn(
    (_input: RequestInfo | URL, init?: RequestInit) =>
      new Promise<Response>((_resolve, reject) => {
        const signal = init?.signal;
        if (!signal) return;
        signal.addEventListener('abort', () => {
          reject(new DOMException('The operation was aborted.', 'AbortError'));
        });
      }),
  ) as unknown as typeof fetch;
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('apiRequest timeouts', () => {
  it('gives up on a hung request and says the server did not answer', async () => {
    vi.useFakeTimers();
    const fetchImpl = hangingFetch();

    const pending = apiRequest('/api/sessions/s_ab12', { fetchImpl });
    // Attach the rejection handler before the clock moves, or the rejection is
    // reported as unhandled.
    const settled = pending.catch((error: unknown) => error);

    await vi.advanceTimersByTimeAsync(30_001);
    const error = await settled;

    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).message).toBe(
      'The server did not answer within 30s (/api/sessions/s_ab12).',
    );
    expect((error as ApiError).code).toBe('timeout');
    // A timeout is not a network failure: the server was reachable, just silent.
    expect((error as ApiError).isNetwork).toBe(false);
  });

  it("still lets the caller's own abort through untouched", async () => {
    const fetchImpl = hangingFetch();
    const controller = new AbortController();

    const pending = apiRequest('/api/sessions/s_ab12', {
      fetchImpl,
      signal: controller.signal,
    });
    const settled = pending.catch((error: unknown) => error);

    controller.abort();
    const error = await settled;

    // Callers rely on recognising their own cancellation, so it must not be
    // rewritten into a timeout or a network error.
    expect(error).toBeInstanceOf(DOMException);
    expect((error as DOMException).name).toBe('AbortError');
  });

  it('does not time out a request that answers in time', async () => {
    vi.useFakeTimers();
    const fetchImpl = vi.fn(async () =>
      new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    ) as unknown as typeof fetch;

    const pending = apiRequest<{ ok: boolean }>('/api/health', { fetchImpl });
    await vi.advanceTimersByTimeAsync(1_000);
    await expect(pending).resolves.toEqual({ ok: true });
  });
});
