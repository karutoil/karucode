import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { useRef } from 'preact/hooks';
import * as actions from '../state/actions';
import { resetAppState } from '../test/fixtures';
import {
  MAX_RESULT_PANEL_WIDTH,
  MIN_CONVERSATION_WIDTH,
  MIN_RESULT_PANEL_WIDTH,
  RESULT_SEPARATOR_WIDTH,
  ResultResizer,
} from './ResultResizer';

const MAX_AT_1200 = 1200 - MIN_CONVERSATION_WIDTH - RESULT_SEPARATOR_WIDTH;

function Harness({ projectId = 'p_one', preferredWidth }: { projectId?: string | null; preferredWidth?: number }) {
  const containerRef = useRef<HTMLDivElement>(null);
  return (
    <div data-testid="split" ref={containerRef}>
      <div class="chat-panel" />
      <ResultResizer containerRef={containerRef} projectId={projectId} preferredWidth={preferredWidth} />
      <aside class="inspector" />
    </div>
  );
}

function geometry(width = 1200, resultWidth = 600): { split: HTMLElement; separator: HTMLElement } {
  const split = screen.getByTestId('split');
  const separator = screen.getByRole('separator', { name: 'Resize results panel' });
  const box = (right: number, left: number) => ({
    x: left, y: 0, left, top: 0, right, bottom: 800, width: right - left, height: 800, toJSON: () => ({}),
  }) as DOMRect;
  split.getBoundingClientRect = () => box(width, 0);
  const result = split.querySelector<HTMLElement>('.inspector')!;
  result.getBoundingClientRect = () => box(width, width - resultWidth);
  let captured = false;
  separator.setPointerCapture = vi.fn(() => { captured = true; });
  separator.releasePointerCapture = vi.fn(() => { captured = false; });
  separator.hasPointerCapture = vi.fn(() => captured);
  return { split, separator };
}

function pointer(
  target: HTMLElement,
  type: string,
  init: { pointerId: number; pointerType: 'mouse' | 'touch'; clientX: number; isPrimary?: boolean; button?: number },
): void {
  // jsdom lacks native `onpointer*` properties. Preact 10 registered CamelCase
  // handlers as expandos there (findable with an `in` check); preact 11 attaches
  // real listeners for the lowercase event types, so dispatch those directly.
  const event = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    clientX: init.clientX,
    button: init.button ?? 0,
  });
  Object.defineProperties(event, {
    pointerId: { value: init.pointerId },
    pointerType: { value: init.pointerType },
    isPrimary: { value: init.isPrimary ?? true },
  });
  fireEvent(target, event);
}

describe('ResultResizer', () => {
  beforeEach(() => {
    resetAppState();
    vi.spyOn(actions, 'saveProjectResultPanelWidth').mockResolvedValue(true);
  });

  afterEach(() => {
    resetAppState();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('keeps the automatic proportional layout until the user chooses a width', () => {
    render(<Harness />);
    const { split } = geometry();
    fireEvent(window, new Event('resize'));
    expect(split.style.getPropertyValue('--workbench-result-w')).toBe('');
    expect(screen.getByRole('separator').getAttribute('aria-valuenow')).toBe('600');
  });

  it('drags with a mouse using pointer capture and saves only on release', async () => {
    render(<Harness preferredWidth={600} />);
    const { split, separator } = geometry();
    pointer(separator, 'pointerdown', { pointerId: 7, pointerType: 'mouse', clientX: 600 });
    expect(separator.getAttribute('data-dragging')).toBe('true');
    expect(separator.setPointerCapture).toHaveBeenCalledWith(7);
    pointer(separator, 'pointermove', { pointerId: 7, pointerType: 'mouse', clientX: 520 });

    expect(split.style.getPropertyValue('--workbench-result-w')).toBe('680px');
    expect(separator.getAttribute('aria-valuenow')).toBe('680');
    expect(separator.getAttribute('data-dragging')).toBe('true');
    expect(actions.saveProjectResultPanelWidth).not.toHaveBeenCalled();

    pointer(separator, 'pointerup', { pointerId: 7, pointerType: 'mouse', clientX: 520 });
    await waitFor(() => expect(actions.saveProjectResultPanelWidth).toHaveBeenCalledWith('p_one', 680));
    expect(actions.saveProjectResultPanelWidth).toHaveBeenCalledTimes(1);
    expect(separator.hasAttribute('data-dragging')).toBe(false);
    expect(separator.releasePointerCapture).toHaveBeenCalledWith(7);
  });

  it('uses the same bounded drag path for touch', async () => {
    render(<Harness preferredWidth={600} />);
    const { split, separator } = geometry();
    pointer(separator, 'pointerdown', { pointerId: 21, pointerType: 'touch', clientX: 500 });
    pointer(separator, 'pointermove', { pointerId: 21, pointerType: 'touch', clientX: 560 });
    expect(split.style.getPropertyValue('--workbench-result-w')).toBe('540px');
    pointer(separator, 'pointerup', { pointerId: 21, pointerType: 'touch', clientX: 560 });
    await waitFor(() => expect(actions.saveProjectResultPanelWidth).toHaveBeenCalledWith('p_one', 540));
  });

  it('cancels capture without applying a meaningless coordinate or saving', () => {
    render(<Harness preferredWidth={600} />);
    const { split, separator } = geometry();
    pointer(separator, 'pointerdown', { pointerId: 9, pointerType: 'touch', clientX: 600 });
    pointer(separator, 'pointermove', { pointerId: 9, pointerType: 'touch', clientX: 520 });
    expect(split.style.getPropertyValue('--workbench-result-w')).toBe('680px');

    pointer(separator, 'pointercancel', { pointerId: 9, pointerType: 'touch', clientX: 0 });
    expect(split.style.getPropertyValue('--workbench-result-w')).toBe('680px');
    expect(separator.hasAttribute('data-dragging')).toBe(false);
    expect(actions.saveProjectResultPanelWidth).not.toHaveBeenCalled();
  });

  it('supports arrows, Shift steps and Home/End with one coalesced keyboard write', async () => {
    vi.useFakeTimers();
    render(<Harness preferredWidth={600} />);
    const { separator } = geometry();
    fireEvent.keyDown(separator, { key: 'ArrowLeft' });
    expect(separator.getAttribute('aria-valuenow')).toBe('610');
    fireEvent.keyDown(separator, { key: 'ArrowLeft', shiftKey: true });
    expect(separator.getAttribute('aria-valuenow')).toBe('660');
    expect(actions.saveProjectResultPanelWidth).not.toHaveBeenCalled();

    act(() => { vi.advanceTimersByTime(279); });
    expect(actions.saveProjectResultPanelWidth).not.toHaveBeenCalled();
    act(() => { vi.advanceTimersByTime(1); });
    expect(actions.saveProjectResultPanelWidth).toHaveBeenCalledWith('p_one', 660);
    expect(actions.saveProjectResultPanelWidth).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(separator, { key: 'End' });
    expect(separator.getAttribute('aria-valuenow')).toBe(String(MAX_AT_1200));
    fireEvent.keyDown(separator, { key: 'Home' });
    expect(separator.getAttribute('aria-valuenow')).toBe(String(MIN_RESULT_PANEL_WIDTH));
    act(() => { vi.advanceTimersByTime(280); });
    expect(actions.saveProjectResultPanelWidth).toHaveBeenLastCalledWith('p_one', MIN_RESULT_PANEL_WIDTH);
    await act(async () => { await Promise.resolve(); });
  });

  it('accumulates a native keyboard burst even before Preact renders between keydowns', () => {
    vi.useFakeTimers();
    render(<Harness preferredWidth={600} />);
    const { separator } = geometry();
    fireEvent.keyDown(separator, { key: 'Home' });
    for (let index = 0; index < 6; index += 1) {
      separator.dispatchEvent(new KeyboardEvent('keydown', {
        key: 'ArrowLeft', shiftKey: true, bubbles: true, cancelable: true,
      }));
    }
    act(() => { vi.advanceTimersByTime(280); });
    expect(actions.saveProjectResultPanelWidth).toHaveBeenLastCalledWith('p_one', 580);
  });

  it('clamps a stored width to this viewport without writing the temporary clamp', () => {
    render(<Harness preferredWidth={MAX_RESULT_PANEL_WIDTH} />);
    const { split, separator } = geometry();
    fireEvent(window, new Event('resize'));
    expect(separator.getAttribute('aria-valuemax')).toBe(String(MAX_AT_1200));
    expect(separator.getAttribute('aria-valuenow')).toBe(String(MAX_AT_1200));
    expect(split.style.getPropertyValue('--workbench-result-w')).toBe(`${MAX_AT_1200}px`);
    expect(actions.saveProjectResultPanelWidth).not.toHaveBeenCalled();
  });

  it('retains a useful local preview with no project and never issues a write', () => {
    render(<Harness projectId={null} preferredWidth={600} />);
    const { split, separator } = geometry();
    fireEvent.keyDown(separator, { key: 'ArrowRight' });
    expect(split.style.getPropertyValue('--workbench-result-w')).toBe('590px');
    expect(separator.getAttribute('aria-valuetext')).toContain('not saved without a project');
    expect(actions.saveProjectResultPanelWidth).not.toHaveBeenCalled();
  });

  it('flushes a pending keyboard burst on unmount', async () => {
    vi.useFakeTimers();
    const view = render(<Harness preferredWidth={600} />);
    const { separator } = geometry();
    fireEvent.keyDown(separator, { key: 'ArrowRight' });
    view.unmount();
    // preact 11 runs passive-effect cleanup on unmount asynchronously (a
    // microtask later), so the flush is awaited rather than assumed synchronous.
    await act(async () => { await Promise.resolve(); });
    expect(actions.saveProjectResultPanelWidth).toHaveBeenCalledWith('p_one', 590);
  });
});
