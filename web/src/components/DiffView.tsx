import type { JSX } from 'preact';
import { useMemo } from 'preact/hooks';
import { parseUnifiedDiff } from '../lib/diff';

export interface DiffViewProps {
  diff: string;
  truncated?: boolean;
  class?: string;
}

/** Colored unified diff with old/new gutters. Parsing is presentation-only. */
export function DiffView({ diff, truncated = false, class: className }: DiffViewProps): JSX.Element {
  const lines = useMemo(() => parseUnifiedDiff(diff), [diff]);
  if (diff.trim() === '') {
    return <p class="muted small">No diff for this file.</p>;
  }
  return (
    <div class={`diff${className ? ` ${className}` : ''}`} role="group" aria-label="Unified diff">
      <div class="diff-lines">
        {lines.map((line, index) => (
          <div class={`diff-line diff-${line.kind}`} key={index}>
            <span class="diff-no" aria-hidden="true">
              {line.oldNo ?? ''}
            </span>
            <span class="diff-no" aria-hidden="true">
              {line.newNo ?? ''}
            </span>
            <span class="diff-text">{line.text === '' ? '\u00a0' : line.text}</span>
          </div>
        ))}
      </div>
      {truncated ? <p class="truncated-note">The server truncated this diff.</p> : null}
    </div>
  );
}