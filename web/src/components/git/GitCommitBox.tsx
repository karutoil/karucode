import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import type { GitStatus } from '../../api/types';
import { canCommit } from './gitModel';

export interface GitCommitBoxProps {
  variant: 'page' | 'panel';
  status: GitStatus | null;
  branch: string | null;
  /** An action is in flight; the box stays readable but cannot be resubmitted. */
  busy: string | null;
  onCommit: (message: string, options: { amend: boolean }) => Promise<boolean>;
}

/** Ctrl/Cmd+Enter submits, the way every commit box does. */
function isSubmitChord(event: KeyboardEvent): boolean {
  return event.key === 'Enter' && (event.metaKey || event.ctrlKey);
}

/**
 * The message box and its commit button.
 *
 * Two behaviours worth naming, because both surprise people:
 *
 * * with nothing staged, `Commit` stages every change first (`stage_all` on the
 *   request) - a commit button that silently does nothing because the change list
 *   is in the other group is worse than the one extra thing it does;
 * * `Amend` reuses the previous message when the box is empty (`--no-edit`), so
 *   fixing a typo in the last commit is one click, not a retyped message.
 */
export function GitCommitBox({
  variant,
  status,
  branch,
  busy,
  onCommit,
}: GitCommitBoxProps): JSX.Element {
  const [message, setMessage] = useState('');
  const [amend, setAmend] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const textRef = useRef<HTMLTextAreaElement>(null);

  // A different repository is a different commit: keeping the draft would let a
  // message written for one project land in another. The previous key is kept in
  // a ref so this only fires on a real *change* - an effect that also ran on
  // mount would wipe the first characters typed while the status was landing.
  const branchKey = `${branch ?? ''}`;
  const previousBranch = useRef(branchKey);
  useEffect(() => {
    if (previousBranch.current === branchKey) return;
    previousBranch.current = branchKey;
    setMessage('');
    setAmend(false);
  }, [branchKey]);

  const gate = canCommit(message, status, { amend });
  const disabled = submitting || busy !== null || !gate.enabled;

  const submit = async (): Promise<void> => {
    if (disabled) return;
    setSubmitting(true);
    const committed = await onCommit(message, { amend });
    setSubmitting(false);
    if (committed) {
      setMessage('');
      setAmend(false);
      textRef.current?.focus();
    }
  };

  return (
    <form
      class={`gitm-commit gitm-commit-${variant}`}
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <label class="sr-only" for={`gitm-message-${variant}`}>
        Commit message
      </label>
      <textarea
        id={`gitm-message-${variant}`}
        ref={textRef}
        class="gitm-message"
        rows={variant === 'page' ? 3 : 2}
        placeholder={
          branch
            ? `Message (commit on ${branch})`
            : 'Commit message'
        }
        value={message}
        disabled={submitting}
        onInput={(event) => setMessage((event.target as HTMLTextAreaElement).value)}
        onKeyDown={(event) => {
          if (!isSubmitChord(event)) return;
          event.preventDefault();
          void submit();
        }}
      />

      <div class="gitm-commit-row">
        <label class="gitm-amend" title="Rewrite the previous commit instead of adding one">
          <input
            type="checkbox"
            checked={amend}
            onChange={(event) => setAmend((event.target as HTMLInputElement).checked)}
          />
          <span>Amend</span>
        </label>
        <button type="submit" class="gitm-commit-btn" disabled={disabled}>
          {busy === null
            ? amend
              ? 'Amend commit'
              : 'Commit'
            : busy}
        </button>
      </div>

      <p class="gitm-commit-hint" role="status">
        {gate.reason ??
          (amend
            ? 'The previous commit will be rewritten.'
            : status && !status.files.some((file) => file.staged)
              ? 'No staged changes — Commit stages every change first.'
              : `${status?.files.filter((file) => file.staged).length ?? 0} staged file(s) will be committed.`)}
      </p>
    </form>
  );
}