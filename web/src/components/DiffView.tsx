import type { JSX } from 'preact';
import { useMemo } from 'preact/hooks';
import { parseUnifiedDiff, type DiffLine } from '../lib/diff';
import { ArrowRightIcon, FileCodeIcon, FileIcon } from './icons';

export interface DiffViewProps {
  diff: string;
  truncated?: boolean;
  class?: string;
  /** File label for the header. Passing it turns the header on. */
  path?: string;
  /** Force the header when the diff itself carries no file name (approvals). */
  showHeader?: boolean;
  /** How many files the review covers; the footer counts them. */
  fileCount?: number;
  /** Enables the "Review changes" footer action. */
  onReview?: () => void;
}

/** Best-effort file name from git/plain unified diff headers. */
function diffFileName(diff: string): string {
  const header = diff.slice(0, 4000).split('\n');
  for (const raw of header) {
    const git = /^diff --git a\/(.+?) b\/(.+)$/.exec(raw);
    if (git) return git[2] ?? git[1] ?? '';
  }
  for (const raw of header) {
    const plus = /^\+\+\+ (?:b\/)?(.+)$/.exec(raw);
    if (plus && plus[1] && plus[1] !== '/dev/null') return plus[1];
  }
  return '';
}

function countLines(lines: DiffLine[]): { adds: number; dels: number } {
  let adds = 0;
  let dels = 0;
  for (const line of lines) {
    if (line.kind === 'add') adds += 1;
    else if (line.kind === 'del') dels += 1;
  }
  return { adds, dels };
}

/** The gutter number the mock prints: the old side for removals, the new one otherwise. */
function gutterNumber(line: DiffLine): string {
  if (line.kind === 'hunk' || line.kind === 'meta') return '';
  const value = line.kind === 'del' ? line.oldNo : (line.newNo ?? line.oldNo);
  return value === null ? '' : String(value);
}

/** Screen readers get the tone the colour is carrying. */
function spokenKind(kind: DiffLine['kind']): string {
  if (kind === 'add') return 'Added line. ';
  if (kind === 'del') return 'Removed line. ';
  if (kind === 'hunk') return 'Hunk header. ';
  if (kind === 'meta') return 'File header. ';
  return '';
}

/**
 * Coloured unified diff with the mock's chrome: file badge, path, +/− counts,
 * one numbered gutter and a footer that says how many files changed. Parsing is
 * presentation-only: the patch is never applied here.
 */
export function DiffView({
  diff,
  truncated = false,
  class: className,
  path,
  showHeader = false,
  fileCount = 1,
  onReview,
}: DiffViewProps): JSX.Element {
  const lines = useMemo(() => parseUnifiedDiff(diff), [diff]);

  if (diff.trim() === '') {
    return <p class="muted small diff-empty">No diff for this file.</p>;
  }

  const withHeader = showHeader || path !== undefined;
  const name = path && path !== '' ? path : diffFileName(diff);
  const { adds, dels } = countLines(lines);
  const files = Number.isFinite(fileCount) && fileCount > 0 ? Math.round(fileCount) : 1;

  return (
    <div
      class={`diff-preview diff${className ? ` ${className}` : ''}`}
      role="group"
      aria-label="Unified diff"
    >
      {withHeader ? (
        <div class="diff-head">
          <span class="file-badge" aria-hidden="true">
            <FileCodeIcon size={15} />
          </span>
          <span class="diff-name" title={name || 'Changes'}>
            {name || 'Changes'}
          </span>
          <span class="diff-stat" title={`${adds} added, ${dels} removed`}>
            <span>+{adds}</span> <em>−{dels}</em>
          </span>
        </div>
      ) : null}

      <div class="diff-lines">
        {lines.map((line, index) => (
          <div class={`diff-line ${line.kind === 'context' ? 'ctx' : line.kind}`} key={index}>
            <span class="sr-only">{spokenKind(line.kind)}</span>
            <i aria-hidden="true">{gutterNumber(line)}</i>
            <span class="diff-text">{line.text === '' ? '\u00a0' : line.text}</span>
          </div>
        ))}
      </div>

      {truncated ? <p class="truncated-note">The server truncated this diff.</p> : null}

      {withHeader ? (
        <div class="diff-footer">
          <span class="diff-files">
            <FileIcon size={13} /> {files === 1 ? '1 file modified' : `${files} files modified`}
          </span>
          {onReview ? (
            <button type="button" class="review-btn" onClick={onReview}>
              Review changes <ArrowRightIcon size={12} />
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
