import type { JSX } from 'preact';
import { FileIcon, FolderIcon } from './icons';
import type { MentionCandidate } from '../lib/mentions';

export interface FileMentionMenuProps {
  /** DOM id of the listbox; the textarea points `aria-controls` at it. */
  id: string;
  items: readonly MentionCandidate[];
  activeIndex: number;
  query: string;
  /** True while the workspace index is being fetched for the first time. */
  loading?: boolean;
  /** Set when the index could not be loaded; the composer stays usable. */
  error?: string | null;
  onPick: (candidate: MentionCandidate) => void;
  onHover: (index: number) => void;
  /** What the list contains: files by default, projects in the control chat. */
  title?: string;
}

function optionId(id: string, index: number): string {
  return `${id}-option-${index}`;
}

function splitPath(path: string): { dir: string; name: string } {
  const index = path.lastIndexOf('/');
  return index === -1
    ? { dir: '', name: path }
    : { dir: path.slice(0, index + 1), name: path.slice(index + 1) };
}

/**
 * The `@` autocomplete popover.
 *
 * Keyboard travel stays in the textarea (that is the combobox pattern): this
 * component is presentational and only reports pointer interaction. Every row
 * is a listbox option so `aria-activedescendant` on the textarea announces the
 * highlighted file while the user keeps typing.
 */
export function FileMentionMenu({
  id,
  items,
  activeIndex,
  query,
  loading = false,
  error = null,
  onPick,
  onHover,
  title = 'Files in this workspace',
}: FileMentionMenuProps): JSX.Element {
  const empty = items.length === 0;
  return (
    <div class="mention-popover">
      <div class="mention-head">
        <span class="mention-title">{title}</span>
        {query !== '' ? <code class="mention-query">{query}</code> : null}
      </div>

      {loading ? (
        <p class="mention-note" role="status">
          Indexing workspace…
        </p>
      ) : null}

      {error ? (
        <p class="mention-note mention-error" role="status">
          {error}
        </p>
      ) : null}

      {empty && !loading ? (
        <p class="mention-note" role="status">
          {query === ''
            ? 'No files found in this workspace yet.'
            : `No file matches “${query}”. Keep typing to use the path verbatim.`}
        </p>
      ) : null}

      {empty ? null : (
        <ul class="mention-list" id={id} role="listbox" aria-label="Workspace files and folders">
          {items.map((item, index) => {
            const active = index === activeIndex;
            const { dir, name } = splitPath(item.path);
            return (
              <li
                key={item.path}
                id={optionId(id, index)}
                role="option"
                aria-selected={active}
                title={item.path}
                class={`mention-item${active ? ' is-active' : ''}`}
                // Keep the caret in the textarea while clicking.
                onMouseDown={(event) => event.preventDefault()}
                onMouseMove={() => {
                  if (!active) onHover(index);
                }}
                onClick={() => onPick(item)}
              >
                <span class={`mention-icon mention-icon-${item.kind}`} aria-hidden="true">
                  {item.kind === 'dir' ? <FolderIcon size={13} /> : <FileIcon size={13} />}
                </span>
                <span class="mention-path">
                  {dir !== '' ? <span class="mention-dir">{dir}</span> : null}
                  <span class="mention-name">{name}</span>
                </span>
                {item.kind === 'dir' ? <span class="mention-kind">folder</span> : null}
              </li>
            );
          })}
        </ul>
      )}

      <div class="mention-foot" aria-hidden="true">
        <span>↑↓ to move</span>
        <span>Enter / Tab to insert</span>
        <span>Esc to dismiss</span>
      </div>
    </div>
  );
}

/** The DOM id the textarea's `aria-controls`/`aria-activedescendant` refer to. */
export function mentionOptionId(menuId: string, index: number): string {
  return optionId(menuId, index);
}