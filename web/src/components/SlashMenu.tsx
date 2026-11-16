import type { JSX } from 'preact';
import type { CommandSpec } from '../api/types';

export interface SlashMenuProps {
  /** DOM id of the listbox; the textarea points `aria-controls` at it. */
  id: string;
  items: readonly CommandSpec[];
  activeIndex: number;
  /** What has been typed after the `/`. */
  query: string;
  /** True while the catalog is being fetched for the first time. */
  loading?: boolean;
  /** Set when the catalog could not be loaded; the box stays usable. */
  error?: string | null;
  onPick: (command: CommandSpec) => void;
  onHover: (index: number) => void;
}

function optionId(id: string, index: number): string {
  return `${id}-option-${index}`;
}

const CATEGORY_LABEL: Record<CommandSpec['category'], string> = {
  session: 'session',
  context: 'context',
  extend: 'extend',
  inspect: 'inspect',
  meta: 'help',
};

/**
 * The `/` autocomplete popover.
 *
 * Same contract as the `@` menu: keyboard travel stays in the textarea (that is
 * the combobox pattern), so this is presentational and only reports pointer
 * interaction. Every row is a listbox option, so the textarea's
 * `aria-activedescendant` announces the highlighted command without moving focus.
 */
export function SlashMenu({
  id,
  items,
  activeIndex,
  query,
  loading = false,
  error = null,
  onPick,
  onHover,
}: SlashMenuProps): JSX.Element {
  const empty = items.length === 0;
  return (
    <div class="mention-popover slash-popover">
      <div class="mention-head">
        <span class="mention-title">Commands</span>
        {query !== '' ? <code class="mention-query">/{query}</code> : null}
      </div>

      {loading ? (
        <p class="mention-note" role="status">
          Loading commands…
        </p>
      ) : null}

      {error ? (
        <p class="mention-note mention-error" role="status">
          {error}
        </p>
      ) : null}

      {empty && !loading && !error ? (
        <p class="mention-note" role="status">
          {query === ''
            ? 'No commands are available on this server.'
            : `No command matches “${query}”. Press Enter to send it as a message.`}
        </p>
      ) : null}

      {empty ? null : (
        <ul class="mention-list" id={id} role="listbox" aria-label="Slash commands">
          {items.map((command, index) => {
            const active = index === activeIndex;
            return (
              <li
                key={command.name}
                id={optionId(id, index)}
                role="option"
                aria-selected={active}
                title={command.summary}
                class={`mention-item${active ? ' is-active' : ''}`}
                // Keep the caret in the textarea while clicking.
                onMouseDown={(event) => event.preventDefault()}
                onMouseMove={() => {
                  if (!active) onHover(index);
                }}
                onClick={() => onPick(command)}
              >
                <code class="slash-usage">
                  /{command.name}
                  {command.args !== '' ? <span class="slash-args"> {command.args}</span> : null}
                </code>
                <span class="slash-title">{command.title}</span>
                <span class="mention-kind">
                  {command.source === 'skill' ? 'skill' : CATEGORY_LABEL[command.category]}
                </span>
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
export function slashOptionId(menuId: string, index: number): string {
  return optionId(menuId, index);
}