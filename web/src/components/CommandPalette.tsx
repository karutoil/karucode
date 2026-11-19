import { Fragment, type JSX } from 'preact';
import { useEffect, useMemo, useState } from 'preact/hooks';
import { relativeTime } from '../lib/format';
import { isImeComposing } from '../lib/keyboard';
import { matchSegments } from '../lib/paletteMatch';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Overlay } from './Overlay';
import {
  ArrowRightIcon,
  BellIcon,
  ChatIcon,
  PanelIcon,
  PlusIcon,
  pluginPanelIcon,
  SearchIcon,
  SettingsIcon,
  SidebarIcon,
} from './icons';

interface PaletteAction {
  id: string;
  label: string;
  group: string;
  hint?: string;
  icon: JSX.Element;
  run: () => void;
}

/**
 * The part of `text` the query matched, in the accent as ink.
 *
 * A palette that filters without saying *why* a row survived leaves the reader
 * guessing — "sess" matches three rows for three different reasons. Marking the
 * run is the answer, and it is the same substring the filter tested, so the two
 * can never disagree.
 */
function matched(text: string, query: string): JSX.Element | string {
  const segments = matchSegments(text, query);
  if (!segments.some((segment) => segment.hit)) return text;
  return (
    <>
      {segments.map((segment) =>
        segment.hit ? <mark class="palette-mark">{segment.text}</mark> : segment.text,
      )}
    </>
  );
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
        icon: <PlusIcon size={15} />,
        run: () => actions.startDraftSession(),
      },
      {
        id: 'toggle-rail',
        label: narrow
          ? 'Open sessions panel'
          : store.railCollapsed.value
            ? 'Show sessions rail'
            : 'Hide sessions rail',
        group: 'Actions',
        hint: 'Cmd/Ctrl+\\',
        icon: <SidebarIcon size={15} />,
        run: actions.toggleRail,
      },
      {
        id: 'toggle-inspector',
        label: narrow
          ? 'Open workspace results'
          : store.inspectorCollapsed.value
            ? 'Show workspace results'
            : 'Hide workspace results',
        group: 'Actions',
        // No hint: the trailing column holds a chord, an age or a count, and this
        // row has none of the three. It used to read "Results", which restated the
        // label it sat beside.
        icon: <PanelIcon size={15} />,
        run: actions.toggleInspector,
      },
      {
        id: 'open-settings',
        label: 'Open settings',
        group: 'Actions',
        icon: <SettingsIcon size={15} />,
        run: actions.openSettings,
      },
      {
        id: 'open-notifications',
        label: 'Open notifications',
        group: 'Actions',
        hint:
          store.unreadTotal.value > 0
            ? `${store.unreadTotal.value} unread`
            : `${store.notifications.value.length} kept`,
        icon: <BellIcon size={15} />,
        run: actions.openNotificationCenter,
      },
    ];

    for (const session of store.sessions.value) {
      list.push({
        id: `session:${session.id}`,
        label: session.title || 'Untitled session',
        group: 'Sessions',
        hint: `${relativeTime(session.updated_at, now)}${session.running ? ' · running' : ''}`,
        icon: <ChatIcon size={15} />,
        run: () => actions.switchSession(session.id),
      });
    }

    // Commands plugins contributed. They are ordinary palette rows, so arrow
    // keys, Enter and the filter treat them exactly like a built-in one; the
    // group and hint name the plugin so a surprising action is attributable
    // before it runs, not after. When the same action is also bound to a chord,
    // the row shows the chord instead — that is what a person is trying to learn.
    const shortcuts = new Map<string, string>();
    for (const binding of store.pluginUi.value.keybindings) {
      if (binding.action && binding.keys && !shortcuts.has(binding.action)) {
        shortcuts.set(binding.action, binding.keys);
      }
    }
    for (const contribution of store.pluginUi.value.commands) {
      list.push({
        id: contribution.hosted_id,
        label: contribution.label,
        group: 'Plugins',
        hint: (contribution.action ? shortcuts.get(contribution.action) : undefined) ?? contribution.plugin,
        icon: pluginPanelIcon(contribution.icon, { size: 15 }),
        run: () => void actions.runPluginContributions(contribution),
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
    store.pluginUi.value,
  ]);

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (needle === '') return actionsList;
    return actionsList.filter((action) =>
      `${action.group} ${action.label} ${action.hint ?? ''}`.toLowerCase().includes(needle),
    );
  }, [actionsList, query]);

  // How many rows of each group survived the filter: the heading's count is a
  // reading of the list in front of the reader, not of the catalog behind it.
  const groupCounts = new Map<string, number>();
  for (const row of filtered) groupCounts.set(row.group, (groupCounts.get(row.group) ?? 0) + 1);

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
    <Overlay
      open
      title="Command palette"
      onClose={actions.closePalette}
      role="palette"
      class="dialog-palette"
      labelId="palette-title"
      // The header band *is* the search field: a title above it would be a
      // second line saying what the placeholder and the list already say.
      showHeader={false}
      bodyLayout="static"
      footer={
        // One hint line: what the keys do, with the match count read off the
        // group headings and announced once by the live region above. The count
        // used to be printed here as well, which was the third place it
        // appeared (live region, group headings, footer).
        <span class="palette-keys">
          <span>
            <kbd>↑</kbd>
            <kbd>↓</kbd> move
          </span>
          <span>
            <kbd>Enter</kbd> run
          </span>
          <span>
            <kbd>Esc</kbd> close
          </span>
        </span>
      }
    >
      <div class="palette-search">
        <SearchIcon size={17} />
        {/* A raw `<input>`, not `src/ui`'s `Input`, and deliberately: the field is
            a `combobox` for the listbox below (`aria-expanded`,
            `aria-controls`, `aria-activedescendant`), which `Input`'s prop list
            does not forward, and `Input` would add its own border/background
            utilities - utilities outrank the components layer - straight over
            `.palette-input`, which paints no frame of its own because the rail
            above it is the cue. `.palette-input` keeps the geometry; this element
            keeps the ARIA. */}
        <input
          type="text"
          class="palette-input"
          placeholder="Search commands, jump to a view…"
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
            // The keys an IME uses to compose are not the palette's: Enter there
            // commits a candidate, and running the first match on it would jump
            // to another session (or a plugin's command) over a word the reader
            // had not finished. Guarded before the arrow branches for the same
            // reason - the candidate list walks with the arrows.
            if (isImeComposing(event)) return;
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
          <li class="palette-empty" role="presentation">
            <SearchIcon size={18} />
            <p class="palette-empty-title">No matching commands</p>
            <p class="palette-empty-copy">
              Nothing in a name, a group or a shortcut matched. Try a shorter word.
            </p>
          </li>
        ) : (
          filtered.map((action, index) => {
            const showGroup = index === 0 || filtered[index - 1]?.group !== action.group;
            return (
              <Fragment key={action.id}>
                {showGroup ? (
                  <li
                    class="session-group-label palette-group-label"
                    role="presentation"
                    aria-hidden="true"
                  >
                    {action.group}
                    <span>{groupCounts.get(action.group) ?? 0}</span>
                  </li>
                ) : null}
                <li
                  id={`palette-option-${action.id}`}
                  role="option"
                  aria-selected={index === activeIndex}
                  class={`palette-item${index === activeIndex ? ' is-active' : ''}`}
                  onMouseMove={() => setActiveIndex(index)}
                  onClick={() => runAction(action)}
                >
                  <span class="palette-item-icon" aria-hidden="true">
                    {action.icon}
                  </span>
                  <span class="palette-label">{matched(action.label, query)}</span>
                  {action.hint ? (
                    <span class="palette-hint">{action.hint}</span>
                  ) : (
                    <span class="palette-arrow" aria-hidden="true">
                      <ArrowRightIcon size={13} />
                    </span>
                  )}
                </li>
              </Fragment>
            );
          })
        )}
      </ul>
    </Overlay>
  );
}