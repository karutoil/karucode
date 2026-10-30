import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { firstLine } from '../state/transcriptLayout';
import type { ThemeThinking } from '../theme/types';

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
  const preview = firstLine(reasoning, 150);
  const bodyRef = useRef<HTMLDivElement>(null);

  // Follow the newest thought while it streams, so the visible line is current
  // rather than stuck on the first thing the model said.
  useEffect(() => {
    const element = bodyRef.current;
    if (!element || !streaming) return;
    element.scrollTop = element.scrollHeight;
  }, [reasoning, streaming]);

  const showPreview = !open && mode === 'preview' && preview !== '';

  return (
    <details class="thinking" data-mode={mode} open={open}>
      <summary class="thinking-summary">
        <span class="thinking-label">{streaming ? 'Thinking…' : 'Thought'}</span>
        {showPreview ? <span class="thinking-preview">{preview}</span> : null}
        {streaming ? <span class="thinking-live" aria-hidden="true" /> : null}
        {!open && !showPreview ? <span class="thinking-hint">Show reasoning</span> : null}
      </summary>
      <div class="thinking-body" ref={bodyRef}>
        {reasoning}
      </div>
    </details>
  );
}
