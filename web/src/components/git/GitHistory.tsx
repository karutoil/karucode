import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import type { GitCommit, GitStash, GitStatus } from '../../api/types';
import { relativeTime } from '../../lib/format';
import { CommitIcon, LayersIcon, PlusIcon, TrashIcon, UndoIcon } from '../icons';

export interface GitHistoryProps {
  variant: 'page' | 'panel';
  /** `null` while the history has not been asked for yet. */
  commits: GitCommit[] | null;
  stashes: GitStash[] | null;
  status: GitStatus | null;
  selectedSha: string | null;
  busy: boolean;
  /** Open the history on first render (the full-width view has the room). */
  defaultOpen: boolean;
  onLoadHistory: () => void;
  onLoadStashes: () => void;
  onSelectCommit: (commit: GitCommit) => void;
  onStash: (action: 'push', includeUntracked: boolean) => void;
  onStashEntry: (action: 'pop' | 'apply' | 'drop', stash: GitStash) => void;
}

/** The `HEAD -> main` decoration, as a readable chip. */
function refChips(refs: string): string[] {
  return refs
    .split(',')
    .map((ref) => ref.trim())
    .filter((ref) => ref !== '' && ref !== 'HEAD')
    .map((ref) => ref.replace(/^HEAD -> /, ''));
}

function stateSummary(commits: GitCommit[] | null): string {
  if (commits === null) return 'not loaded';
  if (commits.length === 0) return 'no commits yet';
  return `${commits.length}${commits.length === 30 ? '+' : ''} commits`;
}

/**
 * Recent commits plus the stash stack.
 *
 * The history is loaded lazily: the inspector opens on a status probe, and three
 * extra `git` processes on app boot is not a cost an empty commit box earns. The
 * full-width view opens it immediately (`defaultOpen`) because that is where the
 * room is.
 */
export function GitHistory({
  variant,
  commits,
  stashes,
  status,
  selectedSha,
  busy,
  defaultOpen,
  onLoadHistory,
  onLoadStashes,
  onSelectCommit,
  onStash,
  onStashEntry,
}: GitHistoryProps): JSX.Element {
  const [open, setOpen] = useState(defaultOpen);
  const [stashOpen, setStashOpen] = useState(false);
  const dirty = (status?.files.length ?? 0) > 0;

  const toggle = (): void => {
    const next = !open;
    setOpen(next);
    if (next && commits === null) onLoadHistory();
  };

  return (
    <div class={`gitm-history gitm-history-${variant}`}>
      <section class="gitm-group" aria-label="History">
        <header class="gitm-group-head">
          <button
            type="button"
            class="gitm-group-toggle"
            aria-expanded={open}
            onClick={toggle}
            title="Recent commits on this branch"
          >
            <span class="gitm-group-chevron" aria-hidden="true">
              <CommitIcon size={12} />
            </span>
            <span class="gitm-group-label">History</span>
            <span class="gitm-group-note">{stateSummary(commits)}</span>
          </button>
          <span class="gitm-group-actions">
            <button
              type="button"
              class="gitm-act"
              title="Reload the history"
              aria-label="Reload the history"
              disabled={busy}
              onClick={onLoadHistory}
            >
              <UndoIcon size={14} />
            </button>
          </span>
        </header>
        {open ? (
          commits === null ? (
            <p class="gitm-note">Loading the history…</p>
          ) : commits.length === 0 ? (
            <p class="gitm-note">No commits yet — the first commit will appear here.</p>
          ) : (
            <ul class="gitm-rows gitm-commits">
              {commits.map((commit) => (
                <li
                  class={`gitm-row gitm-commit-row${selectedSha === commit.id ? ' is-active' : ''}`}
                  key={commit.id}
                >
                  <button
                    type="button"
                    class="gitm-row-main"
                    aria-pressed={selectedSha === commit.id}
                    onClick={() => onSelectCommit(commit)}
                    title={`${commit.short_id} ${commit.subject}`}
                  >
                    <span class="gitm-badge is-info" aria-hidden="true">
                      <CommitIcon size={11} />
                    </span>
                    <span class="gitm-name">
                      <span class="gitm-file">{commit.subject || '(no message)'}</span>
                      <span class="gitm-dir">
                        {commit.author} · {relativeTime(commit.timestamp * 1000)}
                      </span>
                    </span>
                    <span class="gitm-commit-meta">
                      {refChips(commit.refs).map((ref) => (
                        <span class="gitm-ref" key={ref}>
                          {ref}
                        </span>
                      ))}
                      <code class="gitm-sha">{commit.short_id}</code>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )
        ) : null}
      </section>

      <section class="gitm-group" aria-label="Stashes">
        <header class="gitm-group-head">
          <button
            type="button"
            class="gitm-group-toggle"
            aria-expanded={stashOpen}
            onClick={() => {
              const next = !stashOpen;
              setStashOpen(next);
              if (next && stashes === null) onLoadStashes();
            }}
            title="Stashed working trees"
          >
            <span class="gitm-group-chevron" aria-hidden="true">
              <LayersIcon size={12} />
            </span>
            <span class="gitm-group-label">Stashes</span>
            <span class="gitm-group-note">
              {stashes === null ? 'not loaded' : stashes.length === 0 ? 'empty' : `${stashes.length}`}
            </span>
          </button>
          <span class="gitm-group-actions">
            <button
              type="button"
              class="gitm-act"
              title="Stash the tracked changes"
              aria-label="Stash changes"
              disabled={busy || !dirty}
              onClick={() => onStash('push', false)}
            >
              <PlusIcon size={14} />
            </button>
            <button
              type="button"
              class="gitm-act"
              title="Stash all changes, untracked files included"
              aria-label="Stash all changes including untracked files"
              disabled={busy || !dirty}
              onClick={() => onStash('push', true)}
            >
              <LayersIcon size={14} />
            </button>
          </span>
        </header>
        {stashOpen ? (
          stashes === null ? (
            <p class="gitm-note">Loading the stash list…</p>
          ) : stashes.length === 0 ? (
            <p class="gitm-note">No stashes.</p>
          ) : (
            <ul class="gitm-rows">
              {stashes.map((stash) => (
                <li class="gitm-row" key={stash.ref}>
                  <span class="gitm-row-main as-static">
                    <span class="gitm-badge is-info" aria-hidden="true">
                      <LayersIcon size={11} />
                    </span>
                    <span class="gitm-name">
                      <span class="gitm-file">{stash.subject}</span>
                      <span class="gitm-dir">
                        {stash.ref} · {relativeTime(stash.timestamp * 1000)}
                      </span>
                    </span>
                  </span>
                  <span class="gitm-row-actions">
                    <button
                      type="button"
                      class="gitm-act"
                      title="Restore and drop this stash"
                      aria-label={`Pop ${stash.ref}`}
                      disabled={busy}
                      onClick={() => onStashEntry('pop', stash)}
                    >
                      <UndoIcon size={14} />
                    </button>
                    <button
                      type="button"
                      class="gitm-act is-danger"
                      title="Drop this stash (as a reminder: it cannot be undone here)"
                      aria-label={`Drop ${stash.ref}`}
                      disabled={busy}
                      onClick={() => onStashEntry('drop', stash)}
                    >
                      <TrashIcon size={14} />
                    </button>
                  </span>
                </li>
              ))}
            </ul>
          )
        ) : null}
      </section>
    </div>
  );
}