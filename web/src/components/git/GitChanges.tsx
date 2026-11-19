import type { JSX } from 'preact';
import { useRef, useState } from 'preact/hooks';
import type { GitFile } from '../../api/types';
import { copyText } from '../../lib/clipboard';
import { contextMenuProps, Menu, MenuItem, useMenu } from '../../lib/menu';
import * as store from '../../state/store';
import {
  MinusIcon,
  PlusIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  UndoIcon,
} from '../icons';
import { IconButton, LinkButton } from '../../ui';
import {
  type GitGroup,
  type GitGroupId,
  type GitSelection,
  porcelainCode,
  splitPath,
  statusLabel,
  statusLetter,
  statusTone,
} from './gitModel';

export interface GitChangesProps {
  variant: 'page' | 'panel';
  groups: GitGroup[];
  selection: GitSelection | null;
  /** An action is in flight; the buttons stay visible but are disabled. */
  busy: boolean;
  onSelectFile: (file: GitFile, group: GitGroupId) => void;
  onStage: (paths: string[]) => void;
  onUnstage: (paths: string[]) => void;
  onDiscard: (file: GitFile, group: GitGroupId) => void;
  onStageGroup: (group: GitGroup) => void;
  onUnstageGroup: (group: GitGroup) => void;
  onDiscardGroup: (group: GitGroup) => void;
}

const DISCARDABLE: GitGroupId[] = ['changes', 'untracked', 'conflicts', 'staged'];

function rowKey(file: GitFile, group: GitGroupId): string {
  return `${group}:${file.path}`;
}

/** One file: badge, name over its directory, and the actions for its group.
 *
 *  The row's right-click menu copies the path and nothing else. Stage, unstage
 *  and discard stay out of it: they are one click away on the row itself, and a
 *  destructive action duplicated in a pointer menu is a mis-click waiting to
 *  happen. */
function FileRow({
  file,
  group,
  active,
  busy,
  onSelect,
  onStage,
  onUnstage,
  onDiscard,
}: {
  file: GitFile;
  group: GitGroupId;
  active: boolean;
  busy: boolean;
  onSelect: () => void;
  onStage: () => void;
  onUnstage: () => void;
  onDiscard: () => void;
}): JSX.Element {
  const { name, dir } = splitPath(file.path);
  const tone = statusTone(file);
  const label = statusLabel(file);
  const menu = useMenu();
  // Focus returns to the row's main button: the menu has no kebab of its own, so
  // without this Escape would leave focus wherever the browser last had it.
  const mainRef = useRef<HTMLButtonElement>(null);

  /** The path is selectable text; the menu copies the row's own spelling, which
   *  is what a terminal command wants, and says when the clipboard refused. */
  const copyPath = async (): Promise<void> => {
    const copied = await copyText(file.path);
    store.pushToast(copied ? 'Path copied.' : 'Could not copy the path.',
      copied ? 'success' : 'error');
  };

  return (
    <li
      class={`gitm-row${active ? ' is-active' : ''}`}
      {...contextMenuProps(menu, undefined, () => mainRef.current)}
    >
      <LinkButton
        buttonRef={mainRef}
        tone="muted"
        class="gitm-row-main"
        aria-pressed={active}
        aria-label={`${label} ${file.path}`}
        title={`${file.path} — ${label} (${porcelainCode(file)})`}
        onClick={onSelect}
      >
        <span class={`gitm-badge is-${tone}`} aria-hidden="true">
          {statusLetter(file)}
        </span>
        <span class="gitm-name">
          <span class="gitm-file">{name}</span>
          <span class="gitm-dir">{dir || 'workspace root'}</span>
        </span>
        <span class="gitm-row-label">{label}</span>
      </LinkButton>
      <span class="gitm-row-actions">
        {group === 'staged' ? (
          <IconButton
            size="sm"
            class="gitm-act"
            label={`Unstage ${file.path}`}
            title="Unstage this file"
            testId={`git-unstage-${file.path}`}
            icon={<MinusIcon size={14} />}
            disabled={busy}
            onClick={onUnstage}
          />
        ) : (
          <IconButton
            size="sm"
            class="gitm-act"
            label={`Stage ${file.path}`}
            title="Stage this file"
            testId={`git-stage-${file.path}`}
            icon={<PlusIcon size={14} />}
            disabled={busy}
            onClick={onStage}
          />
        )}
        {DISCARDABLE.includes(group) ? (
          <IconButton
            size="sm"
            class="gitm-act is-danger text-danger"
            label={`Discard changes in ${file.path}`}
            title={
              group === 'untracked'
                ? 'Delete this untracked file'
                : 'Discard the changes in this file'
            }
            testId={`git-discard-${file.path}`}
            icon={<UndoIcon size={14} />}
            disabled={busy}
            onClick={onDiscard}
          />
        ) : null}
      </span>
      <Menu menu={menu} float label={`File actions for ${file.path}`}>
        <MenuItem menu={menu} action={{ label: 'Copy path', onSelect: () => void copyPath() }} />
      </Menu>
    </li>
  );
}

/**
 * The grouped change list: staged, changes, untracked, merge conflicts.
 *
 * The groups are collapsible and each header carries its own bulk action, which
 * is what makes a twenty-file change set usable without twenty clicks.
 */
export function GitChanges({
  variant,
  groups,
  selection,
  busy,
  onSelectFile,
  onStage,
  onUnstage,
  onDiscard,
  onStageGroup,
  onUnstageGroup,
  onDiscardGroup,
}: GitChangesProps): JSX.Element {
  const [collapsed, setCollapsed] = useState<Partial<Record<GitGroupId, boolean>>>({});

  if (groups.length === 0) {
    return (
      <div class="gitm-empty" role="status">
        <p class="gitm-empty-title">Working tree clean</p>
        <p class="gitm-empty-text">
          Nothing is staged or modified. Files the agent changes appear here as it writes them.
        </p>
      </div>
    );
  }

  return (
    <div class={`gitm-changes gitm-changes-${variant}`}>
      {groups.map((group) => {
        const open = !collapsed[group.id];
        const fileSelection = selection?.kind === 'file' ? selection : null;
        return (
          <section class="gitm-group" key={group.id} aria-label={group.label}>
            <header class="gitm-group-head">
              <LinkButton
                tone="muted"
                class="gitm-group-toggle"
                aria-expanded={open}
                title={group.hint}
                onClick={() => setCollapsed((current) => ({ ...current, [group.id]: open }))}
              >
                <span class="gitm-group-chevron" aria-hidden="true">
                  {open ? <ChevronDownIcon size={12} /> : <ChevronRightIcon size={12} />}
                </span>
                <span class="gitm-group-label">{group.label}</span>
                <span class="gitm-group-count">{group.files.length}</span>
              </LinkButton>
              <span class="gitm-group-actions">
                {group.id === 'staged' ? (
                  <IconButton
                    size="sm"
                    class="gitm-act"
                    label={`Unstage all ${group.label}`}
                    title="Unstage everything in this group"
                    testId={`git-unstage-group-${group.id}`}
                    icon={<MinusIcon size={14} />}
                    disabled={busy}
                    onClick={() => onUnstageGroup(group)}
                  />
                ) : (
                  <IconButton
                    size="sm"
                    class="gitm-act"
                    label={`Stage all ${group.label}`}
                    title="Stage everything in this group"
                    testId={`git-stage-group-${group.id}`}
                    icon={<PlusIcon size={14} />}
                    disabled={busy}
                    onClick={() => onStageGroup(group)}
                  />
                )}
                {group.id === 'staged' || group.id === 'changes' || group.id === 'untracked' ? (
                  <IconButton
                    size="sm"
                    class="gitm-act is-danger text-danger"
                    label={`Discard all ${group.label}`}
                    title={`Discard every change in ${group.label}`}
                    testId={`git-discard-group-${group.id}`}
                    icon={<UndoIcon size={14} />}
                    disabled={busy}
                    onClick={() => onDiscardGroup(group)}
                  />
                ) : null}
              </span>
            </header>
            {open ? (
              <ul class="gitm-rows">
                {group.files.map((file) => (
                  <FileRow
                    key={rowKey(file, group.id)}
                    file={file}
                    group={group.id}
                    active={
                      fileSelection !== null &&
                      fileSelection.path === file.path &&
                      fileSelection.staged === (group.id === 'staged') &&
                      // A path in both groups is two rows; only the one the user
                      // opened should look selected.
                      fileSelection.group === group.id
                    }
                    busy={busy}
                    onSelect={() => onSelectFile(file, group.id)}
                    onStage={() => onStage([file.path])}
                    onUnstage={() => onUnstage([file.path])}
                    onDiscard={() => onDiscard(file, group.id)}
                  />
                ))}
              </ul>
            ) : null}
          </section>
        );
      })}
    </div>
  );
}