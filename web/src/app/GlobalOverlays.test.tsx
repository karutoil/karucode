import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/preact';
import { GlobalOverlays } from './GlobalOverlays';
import * as actions from '../state/actions';
import * as store from '../state/store';

/**
 * SH-07: a phone sheet is a place, and Back has to leave it.
 *
 * Nothing in the workspace ever touched `history`, so Android's Back gesture did
 * not close the sheet — it left the app, conversation and all. One entry is
 * pushed per open sheet and `popstate` runs the same close the ✕ and the backdrop
 * run.
 */
describe('GlobalOverlays and the Back gesture', () => {
  afterEach(() => {
    cleanup();
    store.mobileSheet.value = null;
    vi.restoreAllMocks();
  });

  it('pushes exactly one entry per open sheet', () => {
    const push = vi.spyOn(window.history, 'pushState');
    const { rerender } = render(<GlobalOverlays />);
    expect(push).not.toHaveBeenCalled();

    store.mobileSheet.value = 'sessions';
    rerender(<GlobalOverlays />);
    expect(push).toHaveBeenCalledTimes(1);
    expect(push.mock.calls[0]?.[0]).toMatchObject({ karucodeSheet: 'sessions' });

    // A sheet replaced by another sheet is the same step for the reader: the
    // entry is rewritten, not stacked.
    store.mobileSheet.value = 'inspector';
    rerender(<GlobalOverlays />);
    expect(push).toHaveBeenCalledTimes(1);
  });

  it('closes the sheet on Back', () => {
    const closeSheets = vi.spyOn(actions, 'closeSheets');
    store.mobileSheet.value = 'sessions';
    render(<GlobalOverlays />);

    fireEvent(window, new Event('popstate'));
    expect(closeSheets).toHaveBeenCalledTimes(1);
  });
});
