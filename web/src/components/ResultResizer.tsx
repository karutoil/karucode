import type { JSX, RefObject } from 'preact';
import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { TargetedKeyboardEvent, TargetedPointerEvent } from 'preact';
import * as actions from '../state/actions';

export const MIN_RESULT_PANEL_WIDTH = 280;
export const MAX_RESULT_PANEL_WIDTH = 1600;
export const MIN_CONVERSATION_WIDTH = 330;
/** The whole track is a hit target; the visible rule is only its centre line. */
export const RESULT_SEPARATOR_WIDTH = 24;
const KEYBOARD_SAVE_DELAY_MS = 280;
const AUTOMATIC_WIDTH_FALLBACK = 560;

function boundedWidth(width: number, maximum = MAX_RESULT_PANEL_WIDTH): number {
  return Math.round(Math.min(maximum, Math.max(MIN_RESULT_PANEL_WIDTH, width)));
}

/** A finite persisted value; old/missing/garbled payloads mean automatic layout. */
function preferredValue(value: number | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? boundedWidth(value) : null;
}

interface ResultResizerProps {
  containerRef: RefObject<HTMLDivElement | null>;
  projectId: string | null;
  preferredWidth?: number;
  /**
   * Which layout this track belongs to. `dock` is the side-by-side split's
   * separator; `popout` is the same gesture on the left edge of the overlay card,
   * where the ceiling is measured against the covered conversation instead.
   */
  variant?: 'dock' | 'popout';
}

interface DragState {
  pointerId: number;
  startX: number;
  startWidth: number;
  maximum: number;
}

/**
 * The separator between Conversation and Results in the wide Workbench split, and
 * the same gesture on the left edge of the pop-out card.
 *
 * Pointer Events provide one path for mouse, pen and touch. Visual resizes stay
 * local; only pointerrelease or a quiet keyboard burst writes to the project.
 * CSS hides the docked track in the intermediate overlay and phone sheet, where a
 * horizontal split would be harmful rather than merely small; the pop-out variant
 * only exists in the desktop card, which is dragged from its own edge.
 */
export function ResultResizer({
  containerRef,
  projectId,
  preferredWidth,
  variant = 'dock',
}: ResultResizerProps): JSX.Element {
  const popout = variant === 'popout';
  // Native repeats/pointer gaps can run before a render. The latest local choice
  // needs one authoritative synchronous record; the version only requests a draw.
  const local = useRef<{ projectId: string | null; width: number } | null>(null);
  const [, setPreviewVersion] = useState(0);
  const [maximum, setMaximum] = useState(MAX_RESULT_PANEL_WIDTH);
  const [measuredAutomatic, setMeasuredAutomatic] = useState<number | null>(null);
  const drag = useRef<DragState | null>(null);
  const saveTimer = useRef<number | null>(null);
  const pendingSave = useRef<{ projectId: string; width: number } | null>(null);

  const selected = local.current?.projectId === projectId ? local.current.width : preferredValue(preferredWidth);
  const currentWidth = boundedWidth(selected ?? measuredAutomatic ?? AUTOMATIC_WIDTH_FALLBACK, maximum);

  const measure = (): { automatic: number | null; maximum: number } => {
    const container = containerRef.current;
    if (!container) return { automatic: null, maximum: MAX_RESULT_PANEL_WIDTH };
    const box = container.getBoundingClientRect();
    // One ceiling for both presentations: a dragged edge may not leave the
    // conversation narrower than its floor, and a floating card may not cover more
    // of it than the docked split could take. Switching modes therefore never
    // changes how much conversation is visible.
    const availableMaximum = box.width > 0
      ? boundedWidth(box.width - MIN_CONVERSATION_WIDTH - RESULT_SEPARATOR_WIDTH, MAX_RESULT_PANEL_WIDTH)
      : MAX_RESULT_PANEL_WIDTH;
    const result = container.querySelector<HTMLElement>(':scope > .inspector');
    const resultWidth = result?.getBoundingClientRect().width ?? 0;
    const automatic = resultWidth > 0
      ? boundedWidth(resultWidth, availableMaximum)
      : null;
    setMaximum(availableMaximum);
    if (automatic !== null) setMeasuredAutomatic(automatic);
    return { automatic, maximum: availableMaximum };
  };

  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    measure();
    if (typeof ResizeObserver === 'function') {
      const observer = new ResizeObserver(() => { measure(); });
      observer.observe(container);
      const result = container.querySelector(':scope > .inspector');
      if (result) observer.observe(result);
      return () => observer.disconnect();
    }
    // Old engines still get correct pointer/keyboard bounds on the next resize.
    window.addEventListener('resize', measure);
    return () => window.removeEventListener('resize', measure);
  }, [containerRef]);

  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    // The docked split only pins a width once the reader (or their saved project
    // preference) chose one; the pop-out always needs a concrete width, because
    // its card and this handle's fixed offset both read the same variable.
    if (selected === null && !popout) container.style.removeProperty('--workbench-result-w');
    else container.style.setProperty('--workbench-result-w', `${currentWidth}px`);
    return () => {
      if (containerRef.current === container) container.style.removeProperty('--workbench-result-w');
    };
  }, [containerRef, currentWidth, selected, popout]);

  const flushSave = (): void => {
    if (saveTimer.current !== null) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    const pending = pendingSave.current;
    pendingSave.current = null;
    if (pending) void actions.saveProjectResultPanelWidth(pending.projectId, pending.width);
  };

  // Flush a pending keyboard choice when leaving this divider or project. The
  // pending record carries its own project id, so A→B cannot save A's width to B.
  useEffect(() => () => flushSave(), []);
  useEffect(() => {
    if (pendingSave.current && pendingSave.current.projectId !== projectId) flushSave();
  }, [projectId]);

  const persist = (width: number, debounce: boolean): void => {
    if (!projectId) return; // With no project, the useful local preview remains.
    pendingSave.current = { projectId, width };
    if (debounce) {
      if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(flushSave, KEYBOARD_SAVE_DELAY_MS);
    } else {
      flushSave();
    }
  };

  const latestWidth = (maximumWidth: number, automatic = measuredAutomatic): number => {
    const preview = local.current?.projectId === projectId ? local.current.width : null;
    return boundedWidth(preview ?? preferredValue(preferredWidth) ?? automatic ?? currentWidth, maximumWidth);
  };

  const selectWidth = (width: number, maximumWidth: number): number => {
    const next = boundedWidth(width, maximumWidth);
    local.current = { projectId, width: next };
    setPreviewVersion((version) => version + 1);
    return next;
  };

  const widthFromPointer = (event: TargetedPointerEvent<HTMLDivElement>): number | null => {
    const active = drag.current;
    if (!active || active.pointerId !== event.pointerId) return null;
    // The results pane is on the right: dragging left makes it wider.
    return selectWidth(active.startWidth + active.startX - event.clientX, active.maximum);
  };

  const finishPointer = (event: TargetedPointerEvent<HTMLDivElement>, save: boolean): void => {
    const active = drag.current;
    if (!active || active.pointerId !== event.pointerId) return;
    // A cancellation can carry a meaningless (often zero) coordinate. Keep the
    // last valid preview, but only a normal pointerup persists the choice.
    if (save) persist(widthFromPointer(event) ?? latestWidth(maximum), false);
    drag.current = null;
    event.currentTarget.removeAttribute('data-dragging');
    if (event.currentTarget.hasPointerCapture?.(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };

  const onKeyDown = (event: TargetedKeyboardEvent<HTMLDivElement>): void => {
    const geometry = measure();
    const startingWidth = latestWidth(geometry.maximum, geometry.automatic);
    const step = event.shiftKey ? 50 : 10;
    let next: number;
    switch (event.key) {
      case 'ArrowLeft':
        next = startingWidth + step;
        break;
      case 'ArrowRight':
        next = startingWidth - step;
        break;
      case 'Home':
        next = MIN_RESULT_PANEL_WIDTH;
        break;
      case 'End':
        next = geometry.maximum;
        break;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
    persist(selectWidth(next, geometry.maximum), true);
  };

  return (
    <div
      class={popout ? 'result-resizer result-resizer-popout' : 'result-resizer'}
      role="separator"
      tabIndex={0}
      aria-label="Resize results panel"
      aria-controls="workbench-results"
      aria-orientation="vertical"
      aria-valuemin={MIN_RESULT_PANEL_WIDTH}
      aria-valuemax={maximum}
      aria-valuenow={currentWidth}
      aria-valuetext={`${currentWidth} pixels${projectId ? '' : ', not saved without a project'}`}
      title="Resize results: drag, touch, or focus and use Left/Right; Shift moves faster; Home/End set bounds"
      onBlur={() => {
        if (drag.current === null) flushSave();
      }}
      onKeyDown={onKeyDown}
      onPointerDown={(event) => {
        if (!event.isPrimary || (event.pointerType === 'mouse' && event.button !== 0)) return;
        const geometry = measure();
        event.preventDefault();
        event.currentTarget.focus();
        event.currentTarget.setPointerCapture?.(event.pointerId);
        event.currentTarget.setAttribute('data-dragging', 'true');
        drag.current = {
          pointerId: event.pointerId,
          startX: event.clientX,
          startWidth: latestWidth(geometry.maximum, geometry.automatic),
          maximum: geometry.maximum,
        };
      }}
      onPointerMove={(event) => {
        if (drag.current?.pointerId !== event.pointerId) return;
        event.preventDefault();
        widthFromPointer(event);
      }}
      onPointerUp={(event) => finishPointer(event, true)}
      onPointerCancel={(event) => finishPointer(event, false)}
      onLostPointerCapture={(event) => finishPointer(event, false)}
    />
  );
}
