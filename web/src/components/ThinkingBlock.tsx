import type { JSX } from 'preact';

export interface ThinkingBlockProps {
  reasoning: string;
  streaming: boolean;
}

/** Reasoning is untrusted text; render it as text, never as Markdown/HTML. */
export function ThinkingBlock({ reasoning, streaming }: ThinkingBlockProps): JSX.Element {
  return (
    <details class="thinking">
      <summary class="thinking-summary">
        <span>{streaming ? 'Thinking…' : 'Thinking'}</span>
        {streaming ? <span class="thinking-live" aria-hidden="true" /> : null}
      </summary>
      <div class="thinking-body">{reasoning}</div>
    </details>
  );
}