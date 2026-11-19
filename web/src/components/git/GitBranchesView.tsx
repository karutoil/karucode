import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import type { GitBranch, GitRemoteBranch, GitStash, GitStatus, GitTag } from '../../api/types';
import { relativeTime } from '../../lib/format';
import {
  BranchIcon,
  CheckIcon,
  EditIcon,
  LayersIcon,
  PlusIcon,
  TagIcon,
  TrashIcon,
  UndoIcon,
  UploadIcon,
} from '../icons';
import { Button, Checkbox, IconButton, Input, LinkButton } from '../../ui';
import { branchNameProblem } from './gitModel';
import type { ConfirmRequest } from './GitConfirmDialog';

export interface GitBranchesViewProps {
  status: GitStatus | null;
  branches: GitBranch[];
  remoteBranches: GitRemoteBranch[];
  tags: GitTag[] | null;
  stashes: GitStash[] | null;
  busy: boolean;
  onLoadTags: () => void;
  onLoadStashes: () => void;
  onCheckout: (name: string, options?: { track?: boolean }) => Promise<boolean>;
  onCreateBranch: (name: string, options?: { checkout?: boolean; start?: string }) => Promise<boolean>;
  onRenameBranch: (oldName: string, newName: string) => Promise<boolean>;
  onDeleteBranch: (name: string, options?: { force?: boolean }) => Promise<boolean>;
  onMerge: (branch: string) => Promise<boolean>;
  onRebase: (branch: string) => Promise<boolean>;
  onManageTag: (body: {
    action: 'create' | 'delete' | 'push';
    name: string;
    message?: string;
  }) => Promise<boolean>;
  onStash: (
    action: 'push' | 'pop' | 'apply' | 'drop',
    options?: { index?: number; message?: string; includeUntracked?: boolean },
  ) => Promise<boolean>;
  onInspectStash: (stash: GitStash) => void;
  onRequestConfirm: (request: ConfirmRequest) => void;
}

export function GitBranchesView({
  status,
  branches,
  remoteBranches,
  tags,
  stashes,
  busy,
  onLoadTags,
  onLoadStashes,
  onCheckout,
  onCreateBranch,
  onRenameBranch,
  onDeleteBranch,
  onMerge,
  onRebase,
  onManageTag,
  onStash,
  onInspectStash,
  onRequestConfirm,
}: GitBranchesViewProps): JSX.Element {
  const [filter, setFilter] = useState('');
  const [newBranch, setNewBranch] = useState('');
  const [renaming, setRenaming] = useState<{ old: string; next: string } | null>(null);

  const [newTag, setNewTag] = useState('');
  const [tagMessage, setTagMessage] = useState('');

  const [stashMessage, setStashMessage] = useState('');
  const [stashUntracked, setStashUntracked] = useState(false);

  useEffect(() => {
    onLoadTags();
    onLoadStashes();
  }, [onLoadTags, onLoadStashes]);

  const needle = filter.trim().toLowerCase();
  const visibleLocal = needle
    ? branches.filter((b) => b.name.toLowerCase().includes(needle))
    : branches;
  const visibleRemote = needle
    ? remoteBranches.filter((b) => b.name.toLowerCase().includes(needle))
    : remoteBranches;
  const tagList = tags ?? [];
  const stashList = stashes ?? [];
  const branchProblem = newBranch.trim() === '' ? null : branchNameProblem(newBranch);

  return (
    <div class="gitm-pane-scroll">
      <div class="gitm-two-col">
        <section class="gitm-card" aria-label="Branches">
          <div class="gitm-card-head">
            <div>
              <h3 class="gitm-card-title">
                <BranchIcon size={15} /> Branches
              </h3>
              <p class="gitm-card-sub">
                {branches.length} local · {remoteBranches.length} remote
              </p>
            </div>
            <Input
              type="search"
              class="gitm-input gitm-search-compact"
              placeholder="Filter branches…"
              aria-label="Filter branches"
              value={filter}
              testId="git-branch-filter"
              onInput={(event) => setFilter((event.target as HTMLInputElement).value)}
            />
          </div>

          <form
            class="gitm-inline-create"
            onSubmit={(event) => {
              event.preventDefault();
              if (busy || branchProblem !== null || newBranch.trim() === '') return;
              void onCreateBranch(newBranch.trim(), { checkout: true }).then((ok) => {
                if (ok) setNewBranch('');
              });
            }}
          >
            <Input
              type="text"
              class="gitm-input"
              placeholder="New branch name (creates & switches)"
              aria-label="Create branch name"
              value={newBranch}
              disabled={busy}
              testId="git-branch-new-name"
              onInput={(event) => setNewBranch((event.target as HTMLInputElement).value)}
            />
            <Button
              type="submit"
              variant="primary"
              size="sm"
              icon={<PlusIcon size={13} />}
              testId="git-branch-create"
              disabled={busy || branchProblem !== null || newBranch.trim() === ''}
            >
              Create
            </Button>
          </form>
          {branchProblem ? (
            <p class="gitm-field-error" role="status">
              {branchProblem}
            </p>
          ) : null}

          <ul class="gitm-ref-list" aria-label="Local branches">
            {visibleLocal.map((branch) => {
              const isCurrent = branch.current || branch.name === status?.branch;
              return (
                <li class={`gitm-ref-row${isCurrent ? ' is-current' : ''}`} key={branch.name}>
                  <div class="gitm-ref-main">
                    <div class="gitm-ref-title">
                      <span class="gitm-ref-name">{branch.name}</span>
                      {isCurrent ? <span class="gitm-ref">current</span> : null}
                      {branch.upstream ? (
                        <span class="gitm-chip">
                          {branch.upstream}
                          {branch.ahead ? ` ↑${branch.ahead}` : ''}
                          {branch.behind ? ` ↓${branch.behind}` : ''}
                        </span>
                      ) : null}
                    </div>
                    {renaming?.old === branch.name ? (
                      <form
                        class="gitm-inline-edit"
                        onSubmit={(event) => {
                          event.preventDefault();
                          if (busy || renaming.next.trim() === '') return;
                          void onRenameBranch(branch.name, renaming.next.trim()).then((ok) => {
                            if (ok) setRenaming(null);
                          });
                        }}
                      >
                        <Input
                          type="text"
                          class="gitm-input"
                          aria-label={`Rename ${branch.name}`}
                          value={renaming.next}
                          disabled={busy}
                          testId={`git-branch-rename-input-${branch.name}`}
                          onInput={(event) =>
                            setRenaming({
                              old: branch.name,
                              next: (event.target as HTMLInputElement).value,
                            })
                          }
                        />
                        <Button
                          type="submit"
                          variant="primary"
                          size="sm"
                          testId="git-branch-rename-save"
                          disabled={busy}
                        >
                          Rename
                        </Button>
                        <Button
                          variant="secondary"
                          size="sm"
                          testId="git-branch-rename-cancel"
                          onClick={() => setRenaming(null)}
                        >
                          Cancel
                        </Button>
                      </form>
                    ) : (
                      <span class="gitm-ref-meta">
                        <code>{branch.commit}</code> · {branch.subject || '(no subject)'} ·{' '}
                        {relativeTime(branch.timestamp * 1000)}
                      </span>
                    )}
                  </div>
                  <div class="gitm-ref-actions">
                    {!isCurrent ? (
                      <>
                        <Button
                          variant="secondary"
                          size="sm"
                          testId={`git-branch-switch-${branch.name}`}
                          disabled={busy}
                          onClick={() => void onCheckout(branch.name)}
                        >
                          Switch
                        </Button>
                        <Button
                          variant="secondary"
                          size="sm"
                          testId={`git-branch-merge-${branch.name}`}
                          disabled={busy}
                          title={`Merge ${branch.name} into ${status?.branch ?? 'HEAD'}`}
                          onClick={() => void onMerge(branch.name)}
                        >
                          Merge
                        </Button>
                        <Button
                          variant="secondary"
                          size="sm"
                          testId={`git-branch-rebase-${branch.name}`}
                          disabled={busy}
                          title={`Rebase ${status?.branch ?? 'HEAD'} onto ${branch.name}`}
                          onClick={() => void onRebase(branch.name)}
                        >
                          Rebase
                        </Button>
                      </>
                    ) : null}
                    <IconButton
                      size="sm"
                      class="gitm-act"
                      label={`Rename branch ${branch.name}`}
                      title="Rename branch"
                      testId={`git-branch-rename-${branch.name}`}
                      icon={<EditIcon size={13} />}
                      disabled={busy}
                      onClick={() => setRenaming({ old: branch.name, next: branch.name })}
                    />
                    {!isCurrent ? (
                      <IconButton
                        size="sm"
                        class="gitm-act is-danger text-danger"
                        label={`Delete branch ${branch.name}`}
                        title="Delete branch"
                        testId={`git-branch-delete-${branch.name}`}
                        icon={<TrashIcon size={13} />}
                        disabled={busy}
                        onClick={() =>
                          onRequestConfirm({
                            title: `Delete branch ${branch.name}?`,
                            description: 'Removes the local branch ref.',
                            confirmLabel: 'Force delete',
                            note: 'If the branch has unmerged commits, force delete removes them from this ref.',
                            onConfirm: async () => {
                              await onDeleteBranch(branch.name, { force: true });
                            },
                          })
                        }
                      />
                    ) : null}
                  </div>
                </li>
              );
            })}
          </ul>

          {visibleRemote.length > 0 ? (
            <div class="gitm-subsection">
              <h4 class="gitm-label">Remote branches</h4>
              <ul class="gitm-ref-list" aria-label="Remote branches">
                {visibleRemote.map((rb) => (
                  <li class="gitm-ref-row" key={rb.name}>
                    <div class="gitm-ref-main">
                      <span class="gitm-ref-name">{rb.name}</span>
                      <span class="gitm-ref-meta">
                        <code>{rb.commit}</code> · {rb.subject || '(no subject)'}
                      </span>
                    </div>
                    <div class="gitm-ref-actions">
                      <Button
                        variant="secondary"
                        size="sm"
                        testId={`git-remote-branch-checkout-${rb.name}`}
                        disabled={busy}
                        onClick={() => void onCheckout(rb.name, { track: true })}
                      >
                        Checkout
                      </Button>
                    </div>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
        </section>

        <div class="gitm-col-stack">
          <section class="gitm-card" aria-label="Tags">
            <div class="gitm-card-head">
              <div>
                <h3 class="gitm-card-title">
                  <TagIcon size={15} /> Tags
                </h3>
                <p class="gitm-card-sub">{tagList.length} tags</p>
              </div>
            </div>
            <form
              class="gitm-inline-create"
              onSubmit={(event) => {
                event.preventDefault();
                if (busy || newTag.trim() === '') return;
                void onManageTag({
                  action: 'create',
                  name: newTag.trim(),
                  message: tagMessage.trim() || undefined,
                }).then((ok) => {
                  if (ok) {
                    setNewTag('');
                    setTagMessage('');
                  }
                });
              }}
            >
              <Input
                type="text"
                class="gitm-input"
                placeholder="v1.0.0"
                aria-label="New tag name"
                value={newTag}
                disabled={busy}
                testId="git-tag-new-name"
                onInput={(event) => setNewTag((event.target as HTMLInputElement).value)}
              />
              <Input
                type="text"
                class="gitm-input"
                placeholder="Annotation message (optional)"
                aria-label="Tag annotation message"
                value={tagMessage}
                disabled={busy}
                testId="git-tag-new-message"
                onInput={(event) => setTagMessage((event.target as HTMLInputElement).value)}
              />
              <Button
                type="submit"
                variant="primary"
                size="sm"
                icon={<PlusIcon size={13} />}
                testId="git-tag-create"
                disabled={busy || newTag.trim() === ''}
              >
                Tag HEAD
              </Button>
            </form>
            {tagList.length === 0 ? (
              <p class="gitm-hint">No tags yet.</p>
            ) : (
              <ul class="gitm-ref-list">
                {tagList.map((tag) => (
                  <li class="gitm-ref-row" key={tag.name}>
                    <div class="gitm-ref-main">
                      <div class="gitm-ref-title">
                        <span class="gitm-ref-name">{tag.name}</span>
                        {tag.annotated ? <span class="gitm-chip">annotated</span> : null}
                      </div>
                      <span class="gitm-ref-meta">
                        <code>{tag.commit}</code>
                        {tag.subject ? ` · ${tag.subject}` : ''}
                      </span>
                    </div>
                    <div class="gitm-ref-actions">
                      <IconButton
                        size="sm"
                        class="gitm-act"
                        label={`Push tag ${tag.name}`}
                        title={`Push tag ${tag.name}`}
                        testId={`git-tag-push-${tag.name}`}
                        icon={<UploadIcon size={13} />}
                        disabled={busy}
                        onClick={() => void onManageTag({ action: 'push', name: tag.name })}
                      />
                      <IconButton
                        size="sm"
                        class="gitm-act is-danger text-danger"
                        label={`Delete tag ${tag.name}`}
                        title={`Delete tag ${tag.name}`}
                        testId={`git-tag-delete-${tag.name}`}
                        icon={<TrashIcon size={13} />}
                        disabled={busy}
                        onClick={() => void onManageTag({ action: 'delete', name: tag.name })}
                      />
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </section>

          <section class="gitm-card" aria-label="Stashes">
            <div class="gitm-card-head">
              <div>
                <h3 class="gitm-card-title">
                  <LayersIcon size={15} /> Stashes
                </h3>
                <p class="gitm-card-sub">{stashList.length} stashed</p>
              </div>
            </div>
            <form
              class="gitm-subform"
              onSubmit={(event) => {
                event.preventDefault();
                if (busy || (status?.files.length ?? 0) === 0) return;
                void onStash('push', {
                  message: stashMessage.trim() || undefined,
                  includeUntracked: stashUntracked,
                }).then((ok) => {
                  if (ok) setStashMessage('');
                });
              }}
            >
              <div class="gitm-inline-create">
                <Input
                  type="text"
                  class="gitm-input"
                  placeholder="Stash message (optional)"
                  aria-label="Stash message"
                  value={stashMessage}
                  disabled={busy}
                  testId="git-stash-message"
                  onInput={(event) => setStashMessage((event.target as HTMLInputElement).value)}
                />
                <Button
                  type="submit"
                  variant="primary"
                  size="sm"
                  testId="git-stash-create"
                  disabled={busy || (status?.files.length ?? 0) === 0}
                >
                  Stash changes
                </Button>
              </div>
              <Checkbox
                class="gitm-check"
                checked={stashUntracked}
                disabled={busy}
                label="Include untracked files"
                onChange={(checked) => setStashUntracked(checked)}
              />
            </form>

            {stashList.length === 0 ? (
              <p class="gitm-hint">No stashes saved.</p>
            ) : (
              <ul class="gitm-ref-list">
                {stashList.map((entry) => (
                  <li class="gitm-ref-row" key={entry.ref}>
                    <LinkButton
                      tone="muted"
                      class="gitm-stash-inspect"
                      onClick={() => onInspectStash(entry)}
                    >
                      <span class="gitm-ref-name">{entry.ref}</span>
                      <span class="gitm-ref-meta">
                        {entry.subject} · {relativeTime(entry.timestamp * 1000)}
                      </span>
                    </LinkButton>
                    <div class="gitm-ref-actions">
                      <IconButton
                        size="sm"
                        class="gitm-act"
                        label={`Apply ${entry.ref}`}
                        title={`Apply ${entry.ref}`}
                        testId={`git-stash-apply-${entry.ref}`}
                        icon={<CheckIcon size={13} />}
                        disabled={busy}
                        onClick={() => void onStash('apply', { index: entry.index })}
                      />
                      <IconButton
                        size="sm"
                        class="gitm-act"
                        label={`Pop ${entry.ref}`}
                        title={`Pop ${entry.ref}`}
                        testId={`git-stash-pop-${entry.ref}`}
                        icon={<UndoIcon size={13} />}
                        disabled={busy}
                        onClick={() => void onStash('pop', { index: entry.index })}
                      />
                      <IconButton
                        size="sm"
                        class="gitm-act is-danger text-danger"
                        label={`Drop ${entry.ref}`}
                        title={`Drop ${entry.ref}`}
                        testId={`git-stash-drop-${entry.ref}`}
                        icon={<TrashIcon size={13} />}
                        disabled={busy}
                        onClick={() =>
                          onRequestConfirm({
                            title: `Drop ${entry.ref}?`,
                            description: 'Deletes this stash entry permanently.',
                            confirmLabel: 'Drop stash',
                            onConfirm: async () => {
                              await onStash('drop', { index: entry.index });
                            },
                          })
                        }
                      />
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </section>
        </div>
      </div>
    </div>
  );
}
