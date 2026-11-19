import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
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
    const deadline = new AbortController();
    // Drive the native AbortSignal deadline rather than assuming advancing a fake
    // clock schedules the composite signal consistently across runner versions.
    // The signal still flows through the real timeout selection and hung request.
    vi.spyOn(AbortSignal, 'timeout').mockReturnValue(deadline.signal);
    const fetchImpl = hangingFetch();

    const pending = apiRequest('/api/sessions/s_ab12', { fetchImpl });
    // Attach the rejection handler before the deadline fires so it cannot be
    // reported as unhandled.
    const settled = pending.catch((error: unknown) => error);

    deadline.abort(new DOMException('Request timed out', 'TimeoutError'));
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

/**
 * `AbortSignal.any` is Firefox 133+; ESR 128 — the version a self-hosted install
 * is most likely pinned to — does not have it. Every request that carries a
 * signal goes through it, so an unguarded call turned each one into a raw
 * `TypeError` before the `try` block could turn it into an `ApiError`.
 *
 * The jsdom this suite runs in *does* provide it, so each case removes it first
 * and asserts the removal took, or the case would prove nothing.
 */
describe('a browser without AbortSignal.any', () => {
  let restoreAny: (() => void) | null = null;

  beforeEach(() => {
    const descriptor = Object.getOwnPropertyDescriptor(AbortSignal, 'any');
    Reflect.deleteProperty(AbortSignal as unknown as Record<string, unknown>, 'any');
    restoreAny = () => {
      if (descriptor) Object.defineProperty(AbortSignal, 'any', descriptor);
    };
    expect(typeof (AbortSignal as { any?: unknown }).any).toBe('undefined');
  });

  afterEach(() => {
    restoreAny?.();
    restoreAny = null;
  });

  it('keeps the caller\'s signal wired while the body is being read', async () => {
    // Releasing the link when the headers arrive would leave `response.text()`
    // uncovered: on an engine without `AbortSignal.any` the composed signal is
    // fed only by that link, so a stalled body would never abort.
    const controller = new AbortController();
    let abortedDuringBody = false;
    const fetchImpl = vi.fn(async (_path: string, init?: RequestInit) => {
      const signal = init?.signal as AbortSignal;
      return {
        ok: true,
        status: 200,
        text: async () => {
          // Abort while the body is in flight, as a caller or the deadline would.
          abortedDuringBody = true;
          expect(signal.aborted).toBe(false);
          controller.abort();
          expect(signal.aborted).toBe(true);
          return JSON.stringify({ ok: true });
        },
      } as unknown as Response;
    });

    await expect(
      apiRequest<{ ok: boolean }>('/api/health', {
        fetchImpl: fetchImpl as unknown as typeof fetch,
        signal: controller.signal,
      }),
    ).resolves.toEqual({ ok: true });
    expect(abortedDuringBody).toBe(true);
  });

  it('does not leave a listener behind on the caller\'s signal', async () => {
    // A request that completes normally never fires `abort`, so the manual link
    // held one listener per request on a signal the app reuses for its life.
    const fetchImpl = vi.fn(
      async () => new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
    const controller = new AbortController();
    const add = vi.spyOn(controller.signal, 'addEventListener');
    const remove = vi.spyOn(controller.signal, 'removeEventListener');

    await apiRequest('/api/health', {
      fetchImpl: fetchImpl as unknown as typeof fetch,
      signal: controller.signal,
    });

    const added = add.mock.calls.filter(([type]) => type === 'abort').length;
    const removed = remove.mock.calls.filter(([type]) => type === 'abort').length;
    expect(added).toBe(1);
    expect(removed).toBe(added);
  });

  it('still sends the request and returns its body', async () => {
    const fetchImpl = vi.fn(
      async () =>
        new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
    );
    const controller = new AbortController();

    await expect(
      apiRequest<{ ok: boolean }>('/api/health', {
        fetchImpl: fetchImpl as unknown as typeof fetch,
        signal: controller.signal,
      }),
    ).resolves.toEqual({ ok: true });
    expect(fetchImpl).toHaveBeenCalledTimes(1);
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

    expect(error).toBeInstanceOf(DOMException);
    expect((error as DOMException).name).toBe('AbortError');
  });

  it('still reports the request deadline as a timeout', async () => {
    const deadline = new AbortController();
    vi.spyOn(AbortSignal, 'timeout').mockReturnValue(deadline.signal);
    const fetchImpl = hangingFetch();
    const caller = new AbortController();

    const pending = apiRequest('/api/sessions/s_ab12', {
      fetchImpl,
      signal: caller.signal,
    });
    const settled = pending.catch((error: unknown) => error);

    deadline.abort(new DOMException('Request timed out', 'TimeoutError'));
    const error = await settled;

    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).code).toBe('timeout');
  });
});
