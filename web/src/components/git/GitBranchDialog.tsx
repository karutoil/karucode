import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import type { GitBranch } from '../../api/types';
import { relativeTime } from '../../lib/format';
import { Button, Field, IconButton, Inline, Input, LinkButton, Overlay, Stack, Text } from '../../ui';
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
 * An overlay rather than a popover: it holds a filter field and a form, both of
 * which need real focus handling and an Escape that does not close the whole
 * inspector. Deleting asks the manager for a confirmation first - `-D` on an
 * unmerged branch throws commits away.
 *
 * The body is left to own its own scrolling (`bodyLayout="contents"`), the same
 * contract the pre-`Overlay` dialog had: `.gitm-branch-body` is the scroller, and
 * a second one wrapped around it would trap the wheel.
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
  const hintId = 'gitm-new-branch-hint';

  return (
    <Overlay
      open={open}
      title="Branches"
      description={
        detached
          ? 'HEAD is detached. Switching to a branch reattaches it.'
          : `On ${current ?? 'an unknown branch'}.`
      }
      onClose={onClose}
      role="standard"
      class="gitm-branch-dialog"
      bodyLayout="contents"
    >
      <Stack class="gitm-branch-body" gap="5">
        <Input
          type="search"
          placeholder="Filter branches"
          aria-label="Filter branches"
          value={filter}
          testId="branch-filter"
          onInput={(event) => setFilter((event.target as HTMLInputElement).value)}
        />

        <Stack as="ul" gap="1" class="gitm-branch-list" aria-label="Local branches">
          {visible.length === 0 ? (
            <li>
              <Text as="span" size="small" tone="muted" class="gitm-note">
                No branch matches “{filter}”.
              </Text>
            </li>
          ) : (
            visible.map((branch) => {
              const isCurrent = branch.current || branch.name === current;
              return (
                <li class={`gitm-branch-row${isCurrent ? ' is-current' : ''}`} key={branch.name}>
                  <LinkButton
                    class="gitm-branch-switch"
                    disabled={busy || isCurrent}
                    onClick={() => {
                      void onCheckout(branch.name).then((ok) => {
                        if (ok) onClose();
                      });
                    }}
                    title={isCurrent ? 'Already checked out' : `Switch to ${branch.name}`}
                    testId={`branch-switch-${branch.name}`}
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
                  </LinkButton>
                  <IconButton
                    label={`Delete branch ${branch.name}`}
                    // The glyph carries the danger ink: the primitive's own
                    // colour utility is what the cascade would win with, so the
                    // destructive colour has to sit on a child instead.
                    icon={
                      <span class="text-danger">
                        <TrashIcon size={14} />
                      </span>
                    }
                    class="gitm-act is-danger"
                    size="sm"
                    title={isCurrent ? 'The checked-out branch cannot be deleted' : `Delete ${branch.name}`}
                    disabled={busy || isCurrent}
                    onClick={() => onRequestDelete(branch)}
                    testId={`branch-delete-${branch.name}`}
                  />
                </li>
              );
            })
          )}
        </Stack>

        <Stack
          as="form"
          gap="3"
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
          {/* The field and its button share one row, bottom-aligned: the label
              sits above the input, so `end` is what keeps the button on the
              input's own baseline instead of floating beside the label. */}
          <Inline gap="4" align="end" class="gitm-branch-create-row">
            <Field
              label={`New branch from ${current ?? 'HEAD'}`}
              htmlFor="gitm-new-branch"
              class="flex-1 min-w-0"
            >
              {(a11y) => (
                <Input
                  id={a11y.id}
                  type="text"
                  placeholder="feat/my-change"
                  value={draft}
                  aria-describedby={a11y.describedBy ?? hintId}
                  testId="branch-name"
                  invalid={problem !== null}
                  onInput={(event) => setDraft((event.target as HTMLInputElement).value)}
                />
              )}
            </Field>
            <Button
              type="submit"
              variant="primary"
              disabled={busy || creating || draft.trim() === '' || problem !== null}
              testId="branch-create"
            >
              Create &amp; switch
            </Button>
          </Inline>
          <Text as="p" size="tiny" tone="muted" class="gitm-hint" id={hintId} role="status">
            {problem ?? 'The new branch starts at the checked-out commit and is switched to.'}
          </Text>
        </Stack>
      </Stack>
    </Overlay>
  );
}
