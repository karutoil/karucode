/* Regression guard for the test-only post-commit scheduler in `setup.ts`.
 *
 * Preact runs `useEffect` bodies from a queued flush, not during commit. Its
 * default scheduler (`afterNextFrame`) is `setTimeout(…, 35)` racing a
 * `requestAnimationFrame`, and the timer path dereferences the *global*
 * `cancelAnimationFrame` after jsdom has been torn down — which is how the suite
 * used to end with `ReferenceError: cancelAnimationFrame is not defined` reported
 * as an unhandled error against whatever file happened to be running.
 *
 * These tests render with Preact's raw `render()` (deliberately not
 * `@testing-library/preact`, whose `act()` hides the scheduler) and then remove
 * the RAF globals to reproduce that teardown window. They fail if the scheduler
 * in `setup.ts` is removed, if it regresses to a timer, or if it stops being the
 * option Preact actually calls.
 */

import { describe, expect, it, vi } from 'vitest';
import { options, render } from 'preact';
import { useEffect } from 'preact/hooks';
import { act } from 'preact/test-utils';

/** Preact's own `RAF_TIMEOUT`; the fallback timer we must never schedule. */
const PREACT_FRAME_FALLBACK_MS = 35;

function Tick({ onEffect }: { onEffect: () => void }) {
  useEffect(() => {
    onEffect();
  }, [onEffect]);
  return <p data-testid="tick">tick</p>;
}

/** Let every queued microtask run without touching the timer queue. */
function drainMicrotasks(): Promise<void> {
  return new Promise<void>((resolve) => queueMicrotask(resolve));
}

function mount(onEffect: () => void): HTMLElement {
  const host = document.createElement('div');
  document.body.appendChild(host);
  render(<Tick onEffect={onEffect} />, host);
  return host;
}

describe('post-commit effect scheduling (test environment)', () => {
  it('installs a scheduler that Preact calls instead of its frame timer', () => {
    // Not the browser's rAF: Preact only takes the `afterNextFrame` path when
    // this option is unset.
    expect(typeof options.requestAnimationFrame).toBe('function');
  });

  it('runs a raw render’s effects on a microtask and schedules no frame timer', async () => {
    const timer = vi.spyOn(globalThis, 'setTimeout');
    let ran = 0;
    const host = mount(() => {
      ran += 1;
    });

    // A microtask is enough: the effect has to be observable before any timer.
    await drainMicrotasks();
    expect(ran).toBe(1);

    const frameTimers = timer.mock.calls.filter(
      ([, delay]) => delay === PREACT_FRAME_FALLBACK_MS,
    );
    expect(frameTimers).toHaveLength(0);

    host.remove();
  });

  it('keeps the effect alive through the environment teardown that used to throw', async () => {
    const globals = globalThis as unknown as Record<string, unknown>;
    const realRaf = globals.requestAnimationFrame;
    const realCancel = globals.cancelAnimationFrame;
    let ran = 0;
    const host = mount(() => {
      ran += 1;
    });

    // Exactly what vitest's jsdom teardown does after a file's last test: the
    // globals go away while a flush may still be queued.
    delete globals.requestAnimationFrame;
    delete globals.cancelAnimationFrame;
    expect(globals.cancelAnimationFrame).toBeUndefined();
    expect(options.requestAnimationFrame).toBeTypeOf('function');
    try {
      // Longer than the 35 ms fallback: a `setTimeout`-based scheduler would
      // have fired here, thrown `cancelAnimationFrame is not defined`, and left
      // `ran` at 0.
      await new Promise<void>((resolve) => setTimeout(resolve, 120));
    } finally {
      globals.requestAnimationFrame = realRaf;
      globals.cancelAnimationFrame = realCancel;
    }

    expect(ran).toBe(1);
    host.remove();
  });

  it('survives the teardown window even when effects are still queued at file end', async () => {
    const globals = globalThis as unknown as Record<string, unknown>;
    const realRaf = globals.requestAnimationFrame;
    const realCancel = globals.cancelAnimationFrame;
    let ran = 0;
    // Rendered and then immediately abandoned with its flush still pending —
    // the "intermittent" shape of the original failure.
    const host = mount(() => {
      ran += 1;
    });
    delete globals.requestAnimationFrame;
    delete globals.cancelAnimationFrame;
    try {
      await drainMicrotasks();
      await drainMicrotasks();
    } finally {
      globals.requestAnimationFrame = realRaf;
      globals.cancelAnimationFrame = realCancel;
    }
    expect(ran).toBe(1);
    host.remove();
  });

  it('restores the test scheduler after act() swaps in its own queue', async () => {
    const scheduler = options.requestAnimationFrame;
    let ran = 0;
    const host = document.createElement('div');
    document.body.appendChild(host);
    await act(() => {
      render(<Tick onEffect={() => (ran += 1)} />, host);
    });
    // `act()` installs a draining queue and puts ours back on the way out; if it
    // did not, the next raw render would hand effects to `afterNextFrame`.
    expect(options.requestAnimationFrame).toBe(scheduler);
    expect(options.requestAnimationFrame).toBeTypeOf('function');
    expect(ran).toBe(1);
    host.remove();
  });
});