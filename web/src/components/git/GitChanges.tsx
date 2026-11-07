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
      <button
        ref={mainRef}
        type="button"
        class="gitm-row-main"
        aria-pressed={active}
        aria-label={`${label} ${file.path}`}
        onClick={onSelect}
        title={`${file.path} — ${label} (${porcelainCode(file)})`}
      >
        <span class={`gitm-badge is-${tone}`} aria-hidden="true">
          {statusLetter(file)}
        </span>
        <span class="gitm-name">
          <span class="gitm-file">{name}</span>
          <span class="gitm-dir">{dir || 'workspace root'}</span>
        </span>
        <span class="gitm-row-label">{label}</span>
      </button>
      <span class="gitm-row-actions">
        {group === 'staged' ? (
          <button
            type="button"
            class="gitm-act"
            title="Unstage this file"
            aria-label={`Unstage ${file.path}`}
            disabled={busy}
            onClick={onUnstage}
          >
            <MinusIcon size={14} />
          </button>
        ) : (
          <button
            type="button"
            class="gitm-act"
            title="Stage this file"
            aria-label={`Stage ${file.path}`}
            disabled={busy}
            onClick={onStage}
          >
            <PlusIcon size={14} />
          </button>
        )}
        {DISCARDABLE.includes(group) ? (
          <button
            type="button"
            class="gitm-act is-danger"
            title={group === 'untracked' ? 'Delete this untracked file' : 'Discard the changes in this file'}
            aria-label={`Discard changes in ${file.path}`}
            disabled={busy}
            onClick={onDiscard}
          >
            <UndoIcon size={14} />
          </button>
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
              <button
                type="button"
                class="gitm-group-toggle"
                aria-expanded={open}
                onClick={() =>
                  setCollapsed((current) => ({ ...current, [group.id]: open }))
                }
                title={group.hint}
              >
                <span class="gitm-group-chevron" aria-hidden="true">
                  {open ? <ChevronDownIcon size={12} /> : <ChevronRightIcon size={12} />}
                </span>
                <span class="gitm-group-label">{group.label}</span>
                <span class="gitm-group-count">{group.files.length}</span>
              </button>
              <span class="gitm-group-actions">
                {group.id === 'staged' ? (
                  <button
                    type="button"
                    class="gitm-act"
                    title="Unstage everything in this group"
                    aria-label={`Unstage all ${group.label}`}
                    disabled={busy}
                    onClick={() => onUnstageGroup(group)}
                  >
                    <MinusIcon size={14} />
                  </button>
                ) : (
                  <button
                    type="button"
                    class="gitm-act"
                    title="Stage everything in this group"
                    aria-label={`Stage all ${group.label}`}
                    disabled={busy}
                    onClick={() => onStageGroup(group)}
                  >
                    <PlusIcon size={14} />
                  </button>
                )}
                {group.id === 'staged' || group.id === 'changes' || group.id === 'untracked' ? (
                  <button
                    type="button"
                    class="gitm-act is-danger"
                    title={`Discard every change in ${group.label}`}
                    aria-label={`Discard all ${group.label}`}
                    disabled={busy}
                    onClick={() => onDiscardGroup(group)}
                  >
                    <UndoIcon size={14} />
                  </button>
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