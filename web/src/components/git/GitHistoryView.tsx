import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import type { GitBranch, GitCommit } from '../../api/types';
import { relativeTime } from '../../lib/format';
import { CommitIcon, SearchIcon } from '../icons';
import { GitDiffPane } from './GitDiffPane';
import type { GitSelection } from './gitModel';
import type { GitHistoryQuery } from './useGitRepo';
import type { ConfirmRequest } from './GitConfirmDialog';

export interface GitHistoryViewProps {
  commits: GitCommit[] | null;
  branches: GitBranch[];
  projectId: string | null;
  revision: number;
  busy: boolean;
  onLoadHistory: (options?: GitHistoryQuery) => void;
  onCheckoutDetached: (sha: string) => Promise<boolean>;
  onCreateBranchAt: (name: string, start: string) => Promise<boolean>;
  onCreateTagAt: (name: string, target: string) => Promise<boolean>;
  onCherryPick: (sha: string) => Promise<boolean>;
  onRevert: (sha: string) => Promise<boolean>;
  onReset: (target: string, mode: 'soft' | 'mixed' | 'hard', confirm?: boolean) => Promise<boolean>;
  onRequestConfirm: (request: ConfirmRequest) => void;
}

function refChips(refs: string): string[] {
  return refs
    .split(',')
    .map((ref) => ref.trim())
    .filter((ref) => ref !== '' && ref !== 'HEAD')
    .map((ref) => ref.replace(/^HEAD -> /, ''));
}

export function GitHistoryView({
  commits,
  branches,
  projectId,
  revision,
  busy,
  onLoadHistory,
  onCheckoutDetached,
  onCreateBranchAt,
  onCreateTagAt,
  onCherryPick,
  onRevert,
  onReset,
  onRequestConfirm,
}: GitHistoryViewProps): JSX.Element {
  const [search, setSearch] = useState('');
  const [scope, setScope] = useState<'current' | 'all' | string>('current');
  const [selected, setSelected] = useState<GitCommit | null>(null);
  const [branchDraft, setBranchDraft] = useState('');
  const [tagDraft, setTagDraft] = useState('');

  useEffect(() => {
    const query: GitHistoryQuery = {
      limit: 60,
      all: scope === 'all',
      branch: scope !== 'current' && scope !== 'all' ? scope : undefined,
      search: search.trim() || undefined,
    };
    onLoadHistory(query);
  }, [scope, search, onLoadHistory]);

  const selection: GitSelection | null = selected
    ? { kind: 'commit', sha: selected.id, subject: selected.subject }
    : null;

  return (
    <div class="gitm-body">
      <aside class="gitm-side" aria-label="Commit history">
        <div class="gitm-filter-bar">
          <div class="gitm-search-wrap">
            <SearchIcon size={13} />
            <input
              type="search"
              class="gitm-input"
              placeholder="Search commit messages…"
              aria-label="Search commit messages"
              value={search}
              onInput={(event) => setSearch((event.target as HTMLInputElement).value)}
            />
          </div>
          <select
            class="gitm-select"
            aria-label="History branch filter"
            value={scope}
            onChange={(event) => setScope((event.target as HTMLSelectElement).value)}
          >
            <option value="current">Current branch</option>
            <option value="all">All branches</option>
            {branches.map((b) => (
              <option key={b.name} value={b.name}>
                {b.name}
              </option>
            ))}
          </select>
        </div>

        <div class="gitm-side-scroll">
          {commits === null ? (
            <p class="gitm-note">Loading commit history…</p>
          ) : commits.length === 0 ? (
            <p class="gitm-note">No commits match this filter.</p>
          ) : (
            <ul class="gitm-rows gitm-graph-list">
              {commits.map((commit) => {
                const active = selected?.id === commit.id;
                const chips = refChips(commit.refs);
                const isMerge = commit.parents.length > 1;
                return (
                  <li
                    class={`gitm-row gitm-commit-row${active ? ' is-active' : ''}`}
                    key={commit.id}
                  >
                    <button
                      type="button"
                      class="gitm-row-main"
                      aria-pressed={active}
                      onClick={() => setSelected(active ? null : commit)}
                    >
                      <span
                        class={`gitm-graph-node${isMerge ? ' is-merge' : ''}`}
                        aria-hidden="true"
                      >
                        <CommitIcon size={12} />
                      </span>
                      <span class="gitm-name">
                        <span class="gitm-file">{commit.subject || '(no message)'}</span>
                        <span class="gitm-dir">
                          <code>{commit.short_id}</code> · {commit.author} ·{' '}
                          {relativeTime(commit.timestamp * 1000)}
                        </span>
                      </span>
                      {chips.length > 0 ? (
                        <span class="gitm-commit-meta">
                          {chips.slice(0, 2).map((chip) => (
                            <span class="gitm-ref" key={chip}>
                              {chip}
                            </span>
                          ))}
                        </span>
                      ) : null}
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </aside>

      <section class="gitm-main" aria-label="Commit details">
        {selected ? (
          <div class="gitm-commit-actions-bar">
            <button
              type="button"
              class="button button-small"
              disabled={busy}
              onClick={() => void onCheckoutDetached(selected.id)}
            >
              Checkout (detached)
            </button>
            <button
              type="button"
              class="button button-small"
              disabled={busy}
              onClick={() => void onCherryPick(selected.id)}
            >
              Cherry-pick
            </button>
            <button
              type="button"
              class="button button-small"
              disabled={busy}
              onClick={() => void onRevert(selected.id)}
            >
              Revert
            </button>
            <button
              type="button"
              class="button button-small"
              disabled={busy}
              onClick={() => void onReset(selected.short_id, 'soft')}
            >
              Soft reset here
            </button>
            <button
              type="button"
              class="button button-small button-danger"
              disabled={busy}
              onClick={() =>
                onRequestConfirm({
                  title: `Hard reset to ${selected.short_id}?`,
                  description:
                    'Resets the current branch, index, and working tree to this commit. Uncommitted changes are lost.',
                  confirmLabel: 'Hard reset',
                  onConfirm: async () => {
                    await onReset(selected.id, 'hard', true);
                  },
                })
              }
            >
              Hard reset
            </button>

            <form
              class="gitm-inline-mini"
              onSubmit={(event) => {
                event.preventDefault();
                if (busy || branchDraft.trim() === '') return;
                void onCreateBranchAt(branchDraft.trim(), selected.id).then((ok) => {
                  if (ok) setBranchDraft('');
                });
              }}
            >
              <input
                type="text"
                class="gitm-input"
                placeholder="Branch from commit…"
                aria-label="Create branch from commit"
                value={branchDraft}
                disabled={busy}
                onInput={(event) => setBranchDraft((event.target as HTMLInputElement).value)}
              />
              <button
                type="submit"
                class="button button-small"
                disabled={busy || branchDraft.trim() === ''}
              >
                Branch
              </button>
            </form>

            <form
              class="gitm-inline-mini"
              onSubmit={(event) => {
                event.preventDefault();
                if (busy || tagDraft.trim() === '') return;
                void onCreateTagAt(tagDraft.trim(), selected.id).then((ok) => {
                  if (ok) setTagDraft('');
                });
              }}
            >
              <input
                type="text"
                class="gitm-input"
                placeholder="Tag commit…"
                aria-label="Create tag on commit"
                value={tagDraft}
                disabled={busy}
                onInput={(event) => setTagDraft((event.target as HTMLInputElement).value)}
              />
              <button
                type="submit"
                class="button button-small"
                disabled={busy || tagDraft.trim() === ''}
              >
                Tag
              </button>
            </form>
          </div>
        ) : null}

        <GitDiffPane
          variant="page"
          selection={selection}
          projectId={projectId}
          reloadKey={revision}
          busy={busy}
          file={null}
          onStage={() => {}}
          onUnstage={() => {}}
          onDiscard={() => {}}
          onClear={() => setSelected(null)}
        />
      </section>
    </div>
  );
}
