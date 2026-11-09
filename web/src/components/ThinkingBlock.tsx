import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { firstLine } from '../state/transcriptLayout';
import type { ThemeThinking } from '../theme/types';
import { TpsBadge } from './TpsBadge';

export interface ThinkingBlockProps {
  reasoning: string;
  streaming: boolean;
  /**
   * How much reasoning is visible before opening (`theme.thinking`):
   * `collapsed` is the bare chip, `preview` adds the first line of thought, and
   * `expanded` renders it open. A running model always previews, because
   * otherwise the only evidence it is alive is a static word.
   */
  mode?: ThemeThinking;
}

/**
 * How much of a live thought is rendered while it streams.
 *
 * The block is a 280 px scroller pinned to the newest line, so this is what the
 * reader can see anyway. Rendering the *whole* thought on every delta meant
 * re-laying out a growing text block - measured at 120 KB of reasoning, that
 * alone held the tab at 15 fps (the answer's Markdown was the other half). The
 * full text is rendered the moment the thought stops, so nothing is lost: it is
 * only the live view that is a window.
 */
const LIVE_REASONING_CHARS = 6_000;

/**
 * The model's reasoning.
 *
 * The old block was a 10px chip reading "Thinking" and nothing else, which made
 * the one place the agent explains itself the easiest thing on the page to miss.
 * It now carries a preview of the thought itself - the first line, dimmed to
 * secondary text - so a glance at the transcript shows *what* the agent was
 * working out, with the full text one click away.
 *
 * Reasoning is untrusted text; render it as text, never as Markdown/HTML.
 */
export function ThinkingBlock({ reasoning, streaming, mode = 'collapsed' }: ThinkingBlockProps): JSX.Element {
  const open = streaming || mode === 'expanded';
  // A live thought is rendered as its newest window, so what arrives is a
  // replacement of a bounded string instead of an append to an unbounded one.
  const body = streaming && reasoning.length > LIVE_REASONING_CHARS
    ? reasoning.slice(-LIVE_REASONING_CHARS)
    : reasoning;
  // `firstLine` flattens the whole string; it is only ever shown when the block
  // is closed, which a streaming block never is, so the scan is skipped rather
  // than repeated per delta.
  const preview = !open && mode === 'preview' ? firstLine(reasoning, 150) : '';

  const bodyRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  // Follow the newest thought while it streams, so the visible line is current
  // rather than stuck on the first thing the model said.
  //
  // Two costs had to go: the old effect ran per delta (hundreds of times a
  // second) and read `scrollHeight`, which forces a synchronous layout of the
  // text that had just been written. A frame both coalesces a burst into one
  // layout and skips it entirely when the reader has scrolled up to read an
  // earlier line - yanking them back to the bottom is the other half of that
  // bug.
  useEffect(() => {
    const element = bodyRef.current;
    if (!element || !streaming || !pinned.current) return;
    if (typeof requestAnimationFrame !== 'function') {
      element.scrollTop = element.scrollHeight;
      return;
    }
    const frame = requestAnimationFrame(() => {
      element.scrollTop = element.scrollHeight;
    });
    return () => cancelAnimationFrame(frame);
  }, [reasoning, streaming]);

  const showPreview = !open && mode === 'preview' && preview !== '';

  return (
    <details class="thinking" data-mode={mode} open={open}>
      <summary class="thinking-summary">
        <span class="thinking-label">{streaming ? 'Thinking…' : 'Thought'}</span>
        {showPreview ? <span class="thinking-preview">{preview}</span> : null}
        {/* Throughput rides the thought that is arriving, so the rate is beside
            the text it describes instead of in the turn's header above it. */}
        <TpsBadge chars={reasoning.length} working={streaming} source="reasoning" />
        {streaming ? <span class="thinking-live" aria-hidden="true" /> : null}
        {!open && !showPreview ? <span class="thinking-hint">Show reasoning</span> : null}
      </summary>
      <div
        class="thinking-body"
        ref={bodyRef}
        onScroll={(event) => {
          const element = event.currentTarget;
          // "Pinned" means the reader is at the bottom - within a line of it, so
          // a fractional layout height does not unpin them by accident.
          pinned.current =
            element.scrollHeight - element.scrollTop - element.clientHeight <= 24;
        }}
      >
        {body}
      </div>
    </details>
  );
}
