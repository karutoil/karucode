import type { JSX } from 'preact';
import { useEffect, useMemo, useState } from 'preact/hooks';
import { relativeTime } from '../lib/format';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Dialog } from './Dialog';
import { SearchIcon } from './icons';

interface PaletteAction {
  id: string;
  label: string;
  group: string;
  hint?: string;
  run: () => void;
}

export function CommandPalette(): JSX.Element | null {
  const open = store.paletteOpen.value;
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);

  useEffect(() => {
    if (open) {
      setQuery('');
      setActiveIndex(0);
    }
  }, [open]);

  const actionsList = useMemo<PaletteAction[]>(() => {
    const now = store.clock.value;
    const narrow = store.narrowViewport.value;
    const list: PaletteAction[] = [
      {
        id: 'new-session',
        label: 'New session',
        group: 'Actions',
        hint: 'Cmd/Ctrl+N',
        run: () => void actions.createSession(),
      },
      {
        id: 'toggle-rail',
        label: narrow
          ? 'Open sessions panel'
          : store.railCollapsed.value
            ? 'Show sessions rail'
            : 'Hide sessions rail',
        group: 'Actions',
        hint: 'Cmd/Ctrl+B',
        run: actions.toggleRail,
      },
      {
        id: 'toggle-inspector',
        label: narrow
          ? 'Open inspector panel'
          : store.inspectorCollapsed.value
            ? 'Show inspector'
            : 'Hide inspector',
        group: 'Actions',
        hint: 'Cmd/Ctrl+J',
        run: actions.toggleInspector,
      },
      {
        id: 'open-settings',
        label: 'Open settings',
        group: 'Actions',
        run: actions.openSettings,
      },
    ];

    for (const session of store.sessions.value) {
      list.push({
        id: `session:${session.id}`,
        label: session.title || 'Untitled session',
        group: 'Sessions',
        hint: `${relativeTime(session.updated_at, now)}${session.running ? ' · running' : ''}`,
        run: () => actions.switchSession(session.id),
      });
    }
    return list;
  }, [
    open,
    store.sessions.value,
    store.railCollapsed.value,
    store.inspectorCollapsed.value,
    store.narrowViewport.value,
    store.clock.value,
  ]);

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (needle === '') return actionsList;
    return actionsList.filter((action) =>
      `${action.group} ${action.label} ${action.hint ?? ''}`.toLowerCase().includes(needle),
    );
  }, [actionsList, query]);

  useEffect(() => {
    setActiveIndex((current) => Math.min(current, Math.max(0, filtered.length - 1)));
  }, [filtered.length]);

  useEffect(() => {
    setActiveIndex(0);
  }, [query]);

  if (!open) return null;

  const activeAction = filtered[activeIndex];

  const runAction = (action: PaletteAction | undefined): void => {
    if (!action) return;
    actions.closePalette();
    action.run();
  };

  return (
    <Dialog
      open
      title="Command palette"
      onClose={actions.closePalette}
      class="dialog-palette"
      labelId="palette-title"
    >
      <div class="palette-search">
        <SearchIcon size={16} />
        <input
          type="text"
          class="palette-input"
          placeholder="Search actions and sessions…"
          value={query}
          data-autofocus
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-autocomplete="list"
          aria-activedescendant={activeAction ? `palette-option-${activeAction.id}` : undefined}
          aria-label="Search commands"
          onInput={(event) => setQuery(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === 'ArrowDown') {
              event.preventDefault();
              setActiveIndex((current) => Math.max(0, Math.min(filtered.length - 1, current + 1)));
            } else if (event.key === 'ArrowUp') {
              event.preventDefault();
              setActiveIndex((current) => Math.max(0, current - 1));
            } else if (event.key === 'Home') {
              event.preventDefault();
              setActiveIndex(0);
            } else if (event.key === 'End') {
              event.preventDefault();
              setActiveIndex(Math.max(0, filtered.length - 1));
            } else if (event.key === 'Enter') {
              event.preventDefault();
              runAction(activeAction);
            }
          }}
        />
      </div>

      <p class="sr-only" aria-live="polite">
        {filtered.length} result{filtered.length === 1 ? '' : 's'}
      </p>

      <ul class="palette-list" id="palette-list" role="listbox" aria-label="Commands">
        {filtered.length === 0 ? (
          <li class="palette-empty">No matching actions.</li>
        ) : (
          filtered.map((action, index) => {
            const showGroup = index === 0 || filtered[index - 1]?.group !== action.group;
            return (
              <li
                key={action.id}
                id={`palette-option-${action.id}`}
                role="option"
                aria-selected={index === activeIndex}
                class={`palette-option${index === activeIndex ? ' is-active' : ''}`}
                onMouseMove={() => setActiveIndex(index)}
                onClick={() => runAction(action)}
              >
                <span class="palette-group" aria-hidden={!showGroup}>
                  {showGroup ? action.group : ''}
                </span>
                <span class="palette-label">{action.label}</span>
                {action.hint ? <span class="palette-hint">{action.hint}</span> : null}
              </li>
            );
          })
        )}
      </ul>

      <footer class="palette-footer">
        <span>
          <kbd>↑</kbd>
          <kbd>↓</kbd> navigate
        </span>
        <span>
          <kbd>Enter</kbd> run
        </span>
        <span>
          <kbd>Esc</kbd> close
        </span>
      </footer>
    </Dialog>
  );
}