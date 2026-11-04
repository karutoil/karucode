import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import type { GitBranch } from '../../api/types';
import { relativeTime } from '../../lib/format';
import { Dialog } from '../Dialog';
import { BranchIcon, CheckIcon, TrashIcon } from '../icons';
import { branchNameProblem } from './gitModel';

export interface GitBranchDialogProps {
  open: boolean;
  onClose: () => void;
  branches: GitBranch[];
  current: string | null;
  detached: boolean;
  /** An action is in flight: rows stay readable but do nothing until it lands. */
  busy: boolean;
  onCheckout: (name: string) => Promise<boolean>;
  onCreate: (name: string) => Promise<boolean>;
  onRequestDelete: (branch: GitBranch) => void;
}

/**
 * Branch switching, creation and deletion in one place.
 *
 * A dialog rather than a popover: it holds a filter field and a form, both of
 * which need real focus handling and an Escape that does not close the whole
 * inspector. Deleting asks the manager for a confirmation first - `-D` on an
 * unmerged branch throws commits away.
 */
export function GitBranchDialog({
  open,
  onClose,
  branches,
  current,
  detached,
  busy,
  onCheckout,
  onCreate,
  onRequestDelete,
}: GitBranchDialogProps): JSX.Element {
  const [filter, setFilter] = useState('');
  const [draft, setDraft] = useState('');
  const [creating, setCreating] = useState(false);

  const needle = filter.trim().toLowerCase();
  const visible = needle === ''
    ? branches
    : branches.filter((branch) => branch.name.toLowerCase().includes(needle));
  const problem = draft.trim() === '' ? null : branchNameProblem(draft);

  return (
    <Dialog
      open={open}
      title="Branches"
      description={
        detached
          ? 'HEAD is detached. Switching to a branch reattaches it.'
          : `On ${current ?? 'an unknown branch'}.`
      }
      onClose={onClose}
      class="gitm-branch-dialog"
    >
      <div class="gitm-branch-body">
        <input
          class="gitm-input"
          type="search"
          placeholder="Filter branches"
          aria-label="Filter branches"
          value={filter}
          onInput={(event) => setFilter((event.target as HTMLInputElement).value)}
        />

        <ul class="gitm-branch-list" aria-label="Local branches">
          {visible.length === 0 ? (
            <li class="gitm-note">No branch matches “{filter}”.</li>
          ) : (
            visible.map((branch) => {
              const isCurrent = branch.current || branch.name === current;
              return (
                <li class={`gitm-branch-row${isCurrent ? ' is-current' : ''}`} key={branch.name}>
                  <button
                    type="button"
                    class="gitm-branch-switch"
                    disabled={busy || isCurrent}
                    onClick={() => {
                      void onCheckout(branch.name).then((ok) => {
                        if (ok) onClose();
                      });
                    }}
                    title={isCurrent ? 'Already checked out' : `Switch to ${branch.name}`}
                  >
                    <span class="gitm-branch-icon" aria-hidden="true">
                      {isCurrent ? <CheckIcon size={13} /> : <BranchIcon size={13} />}
                    </span>
                    <span class="gitm-branch-text">
                      <span class="gitm-branch-name">{branch.name}</span>
                      <span class="gitm-branch-meta">
                        {branch.subject || '(no commits)'} ·{' '}
                        {branch.timestamp > 0 ? relativeTime(branch.timestamp * 1000) : 'no date'}
                      </span>
                    </span>
                    {branch.upstream ? (
                      <span class="gitm-branch-track" title={`Tracking ${branch.upstream}`}>
                        {branch.ahead > 0 ? `↑${branch.ahead}` : ''}
                        {branch.behind > 0 ? `↓${branch.behind}` : ''}
                        {branch.ahead === 0 && branch.behind === 0 ? 'synced' : ''}
                      </span>
                    ) : null}
                  </button>
                  <button
                    type="button"
                    class="gitm-act is-danger"
                    aria-label={`Delete branch ${branch.name}`}
                    title={isCurrent ? 'The checked-out branch cannot be deleted' : `Delete ${branch.name}`}
                    disabled={busy || isCurrent}
                    onClick={() => onRequestDelete(branch)}
                  >
                    <TrashIcon size={14} />
                  </button>
                </li>
              );
            })
          )}
        </ul>

        <form
          class="gitm-branch-create"
          onSubmit={(event) => {
            event.preventDefault();
            if (problem !== null || busy) return;
            setCreating(true);
            void onCreate(draft.trim()).then((ok) => {
              setCreating(false);
              if (ok) {
                setDraft('');
                onClose();
              }
            });
          }}
        >
          <label class="gitm-label" for="gitm-new-branch">
            New branch from {current ?? 'HEAD'}
          </label>
          <div class="gitm-branch-create-row">
            <input
              id="gitm-new-branch"
              class="gitm-input"
              type="text"
              placeholder="feat/my-change"
              value={draft}
              onInput={(event) => setDraft((event.target as HTMLInputElement).value)}
            />
            <button
              type="submit"
              class="button button-primary"
              disabled={busy || creating || draft.trim() === '' || problem !== null}
            >
              Create &amp; switch
            </button>
          </div>
          <p class="gitm-hint" role="status">
            {problem ?? 'The new branch starts at the checked-out commit and is switched to.'}
          </p>
        </form>
      </div>
    </Dialog>
  );
}