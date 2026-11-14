/**
 * The settings sheet's navigation rail and its search.
 *
 * The sheet used to be a horizontal tab strip over one scrolling panel. Seven
 * labels in a row — "Plugins & MCP" among them — do not say what is behind them,
 * and nothing could be found without opening every one. The rail answers both:
 * each section carries a line of copy saying what it holds, and the search field
 * filters the rail against the *rendered* panels, so a setting added tomorrow is
 * findable the day it is added instead of the day someone remembers to add a
 * keyword.
 *
 * Two facts live here rather than in `SettingsSheet.tsx`:
 *
 * * the section list — its label, the note under it, the pane's caption, and who
 *   owns its values (`This browser` vs `This install`). The sheet renders the
 *   same record as the pane header, so a section cannot be named one way in the
 *   rail and another above its panel.
 * * `searchSettings()` — the matcher, which reads the panels as they are drawn
 *   and returns the sections inside each tab that matched.
 *
 * The rail is a vertical `tablist`: it swaps the panel beside it, which is what
 * ARIA tabs are for, and it keeps the keyboard contract the old strip had
 * (arrows, Home/End, roving `tabIndex`, only the selected tab in the tab order).
 */

import type { JSX } from 'preact';
import { useRef } from 'preact/hooks';
import {
  AgentsIcon,
  BellIcon,
  BoltIcon,
  CpuIcon,
  DownloadIcon,
  LockIcon,
  PaletteIcon,
  SearchIcon,
  SettingsIcon,
  SparkIcon,
} from './icons';

export type SettingsTab =
  | 'general'
  | 'models'
  | 'agent'
  | 'skills'
  | 'access'
  | 'notifications'
  | 'plugins'
  | 'updates';

/**
 * Who owns a tab's values, said in the pane header.
 *
 * It is the question the old sheet answered in prose, in one place only ("Stored
 * in this browser, not in the server config"): whether pressing Save is what
 * commits what you are looking at. `mixed` is a tab that holds both — the
 * notification switches save themselves, the device section never leaves the
 * browser.
 */
export type SettingsScope = 'browser' | 'install' | 'mixed';

export const SETTINGS_SCOPE_LABELS: Record<SettingsScope, string> = {
  browser: 'This browser',
  install: 'This install',
  mixed: 'Install + browser',
};

export interface SettingsSectionMeta {
  id: SettingsTab;
  /** The rail label and the pane's `<h2>`: one spelling for one section. */
  label: string;
  /** The line under the rail label: what is in there, in three or four words. */
  note: string;
  /** The pane's subtitle: the sentence a user arriving cold needs. */
  caption: string;
  scope: SettingsScope;
  icon: JSX.Element;
  /**
   * Words a search should reach this section by, that the rendered panel may
   * not contain (a synonym the user types and the screen never prints).
   *
   * This is deliberately *not* the search index: `searchSettings()` reads the
   * panels. It only widens the synonyms, so it cannot go stale in a way that
   * hides a setting.
   */
  keywords: string;
}

export const SETTINGS_SECTIONS: readonly SettingsSectionMeta[] = [
  {
    id: 'general',
    label: 'Appearance',
    note: 'Theme, transcript, results',
    caption:
      'How the workspace looks and how the conversation is drawn — theme, density, colour mode and the transcript layout.',
    scope: 'browser',
    icon: <PaletteIcon size={15} />,
    keywords: 'colours colors font density typography layout theme studio dark light',
  },
  {
    id: 'models',
    label: 'Models & providers',
    note: 'Endpoints, keys, models',
    caption:
      'The endpoints karucode may call, the key each one uses, and the models it offers.',
    scope: 'install',
    icon: <CpuIcon size={15} />,
    keywords: 'llm api key provider endpoint openrouter models.dev sampling temperature',
  },
  {
    id: 'agent',
    label: 'Agent',
    note: 'Approvals, subagents, limits',
    caption:
      'What the agent may do without asking, and the bounds a run and its subagents work inside.',
    scope: 'install',
    icon: <AgentsIcon size={15} />,
    keywords: 'approval allowlist auto read-only permission subagent concurrency iterations limits timeouts',
  },
  {
    id: 'skills',
    label: 'Skills',
    note: 'Procedures it loads on demand',
    caption:
      'Instruction packs the agent opens only when a task matches their description - where they are found, what of them reaches the prompt, and the ones you add yourself.',
    scope: 'install',
    icon: <SparkIcon size={15} />,
    keywords:
      'skill skill.md procedures playbook prompt context folders auto-detect discover add remove enable disable body',
  },
  {
    id: 'access',
    label: 'Access & sign-in',
    note: 'Sign-in, password, passkeys',
    caption:
      'Who may reach this server, and how they prove it. Anything here changes the login screen, so a change is saved on its own.',
    scope: 'install',
    icon: <LockIcon size={15} />,
    keywords: 'auth login password passkey session tunnel public url trusted proxy security',
  },
  {
    id: 'notifications',
    label: 'Notifications',
    note: 'Run events, delivery, device',
    caption:
      'Which events are worth interrupting you for, on the server and in this browser.',
    scope: 'mixed',
    icon: <BellIcon size={15} />,
    keywords: 'notify alerts sound desktop webhook plugin delivery min run',
  },
  {
    id: 'plugins',
    label: 'Plugins & MCP',
    note: 'Processes, tools, preferences',
    caption:
      'What is running beside the server, what each one contributes, and what of that this browser should show.',
    scope: 'mixed',
    icon: <BoltIcon size={15} />,
    keywords: 'plugin mcp server tools reload rescan extension children processes',
  },
  {
    id: 'updates',
    label: 'Updates',
    note: 'Build, checks, install',
    caption:
      'Which build is running, what the last check found, and whether karucode may install a newer one itself.',
    scope: 'install',
    icon: <DownloadIcon size={15} />,
    keywords: 'version upgrade release changelog build commit auto check manifest',
  },
];

/** The metadata of one section; a hard lookup because the union has no gaps. */
export function settingsSection(id: SettingsTab): SettingsSectionMeta {
  const section = SETTINGS_SECTIONS.find((candidate) => candidate.id === id);
  if (!section) throw new Error(`unknown settings section: ${id}`);
  return section;
}

/**
 * One tab that the query reached.
 *
 * `sections` holds the headings (the panel's `<h3>`s) whose section text
 * contains every word of the query, in document order. A tab can match on its
 * own name without any of its sections doing so ("access" also appears in the
 * caption), and then the rail still shows it — with its normal note.
 */
export interface SettingsSearchResult {
  tab: SettingsTab;
  sections: string[];
}

/** Lowercase, single-spaced: the shape both the query and the page are in. */
function normalize(text: string): string {
  return text.toLowerCase().replace(/\s+/g, ' ').trim();
}

/** Why a section matched: every word of the query appears somewhere inside it. */
function sectionMatches(text: string, words: readonly string[]): boolean {
  return words.every((word) => text.includes(word));
}

/**
 * The sections of one tab that the query reaches, in document order.
 *
 * The corpus is the rendered panel — not a keyword list — so it describes this
 * sheet at this moment. Words are matched separately rather than as one phrase
 * because `textContent` glues sibling elements together (`<h3>Subagents</h3>
 * <span>Same workspace</span>` is one string with no space), and a phrase query
 * would then fail at every heading boundary.
 */
export function matchingSections(tab: SettingsTab, query: string): HTMLElement[] {
  const words = normalize(query).split(' ').filter(Boolean);
  if (words.length === 0 || typeof document === 'undefined') return [];
  const panel = document.getElementById(`settings-panel-${tab}`);
  if (!panel) return [];
  return Array.from(panel.querySelectorAll<HTMLElement>('.settings-section')).filter((block) => {
    const text = normalize(block.textContent ?? '');
    return text !== '' && sectionMatches(text, words);
  });
}

/**
 * Which sections the query reaches, tab by tab.
 *
 * A tab that matches on its own name stays in the rail even when none of its
 * sections do: someone typing "access" is looking for the tab, and the note under
 * the label says what it holds.
 */
export function searchSettings(query: string): SettingsSearchResult[] {
  const words = normalize(query).split(' ').filter(Boolean);
  if (words.length === 0) return [];
  const results: SettingsSearchResult[] = [];
  for (const section of SETTINGS_SECTIONS) {
    const headings: string[] = [];
    for (const block of matchingSections(section.id, query)) {
      const heading = block.querySelector('h3')?.textContent?.trim() ?? '';
      if (heading !== '' && !headings.includes(heading)) headings.push(heading);
    }
    const about = normalize(
      `${section.label} ${section.note} ${section.caption} ${section.keywords}`,
    );
    if (headings.length > 0 || sectionMatches(about, words)) {
      results.push({ tab: section.id, sections: headings });
    }
  }
  return results;
}

/** The line under a matching rail label: which of its sections the query hit. */
function matchNote(sections: readonly string[]): string {
  const [first, second, third, ...rest] = sections;
  if (first === undefined) return '';
  const shown = [first, second, third].filter((title): title is string => title !== undefined);
  return rest.length > 0 ? `${shown.join(' · ')} +${rest.length}` : shown.join(' · ');
}

export interface SettingsNavProps {
  active: SettingsTab;
  /** The live query; empty means "no search, show every section". */
  query: string;
  /** What the query reached, as `searchSettings()` answered it. */
  results: readonly SettingsSearchResult[];
  onQueryChange: (query: string) => void;
  /** Select a tab, and jump to its first matching section when `jump` is set. */
  onSelect: (tab: SettingsTab, jump: boolean) => void;
}

export function SettingsNav({
  active,
  query,
  results,
  onQueryChange,
  onSelect,
}: SettingsNavProps): JSX.Element {
  const listRef = useRef<HTMLDivElement>(null);
  const searching = query.trim() !== '';
  const matches = new Map(results.map((result) => [result.tab, result]));
  const visible = searching
    ? SETTINGS_SECTIONS.filter((section) => matches.has(section.id))
    : SETTINGS_SECTIONS;
  // While a query is active the selected section can be filtered out of the rail
  // even though its panel is still the one on screen.
  const activeHidden = visible.every((section) => section.id !== active);

  /**
   * Arrow keys move the selection, Home/End jump to the ends.
   *
   * Both axes are accepted on purpose: the rail is vertical on a desktop and a
   * scrolling strip on a phone, and a user should not have to know which layout
   * they are looking at to move between sections.
   *
   * While a query is active the selected section can be filtered out of the rail
   * — typing "timeout" hides Appearance, which is still the panel on screen — so
   * an arrow key starts from the first visible row instead of doing nothing.
   */
  const onKeyDown = (event: JSX.TargetedKeyboardEvent<HTMLDivElement>): void => {
    if (visible.length === 0) return;
    const index = visible.findIndex((section) => section.id === active);
    let next = index;
    if (event.key === 'ArrowDown' || event.key === 'ArrowRight') {
      next = index === -1 ? 0 : (index + 1) % visible.length;
    } else if (event.key === 'ArrowUp' || event.key === 'ArrowLeft') {
      next = index === -1 ? visible.length - 1 : (index - 1 + visible.length) % visible.length;
    } else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = visible.length - 1;
    else return;
    event.preventDefault();
    const target = visible[next];
    if (!target) return;
    onSelect(target.id, false);
    listRef.current
      ?.querySelector<HTMLElement>(`#settings-nav-tab-${target.id}`)
      ?.focus();
  };

  const onSearchKeyDown = (event: JSX.TargetedKeyboardEvent<HTMLInputElement>): void => {
    // Escape belongs to the field while it holds a query, and to the sheet when
    // it does not: `data-escape-local` is what tells the dialog's focus trap to
    // keep its hands off. Without it the sheet would close and take the query
    // with it.
    if (event.key === 'Escape' && searching) {
      event.preventDefault();
      onQueryChange('');
      return;
    }
    if (event.key !== 'Enter') return;
    // Always take Enter away from the form: without a match there is nothing to
    // open, and an implicit submit would save the whole sheet because someone
    // pressed Enter in a search field.
    event.preventDefault();
    const first = visible[0];
    if (!first) return;
    onSelect(first.id, searching);
  };

  return (
    <nav class="settings-nav" aria-label="Settings">
      <div class="settings-nav-head">
        <p class="settings-nav-kicker">
          <SettingsIcon size={13} />
          Settings
        </p>
        <label class="side-search settings-search">
          <SearchIcon size={14} />
          <input
            type="search"
            placeholder="Search settings"
            aria-label="Search settings"
            value={query}
            data-escape-local={searching ? '' : undefined}
            onInput={(event) => onQueryChange(event.currentTarget.value)}
            onKeyDown={onSearchKeyDown}
          />
        </label>
        {searching ? (
          <p class="settings-nav-hint" aria-live="polite">
            {visible.length === 0
              ? 'No section matches'
              : `${visible.length} section${visible.length === 1 ? '' : 's'} match · Enter opens the first`}
          </p>
        ) : null}
      </div>

      <div
        ref={listRef}
        class="settings-nav-list"
        role="tablist"
        aria-label="Settings sections"
        aria-orientation="vertical"
        onKeyDown={onKeyDown}
      >
        {visible.map((section) => {
          const selected = section.id === active;
          const match = matches.get(section.id);
          // A row that matched only on its own name (no section inside it did)
          // keeps the note that describes the section: "Open" would say nothing.
          const note =
            searching && match && match.sections.length > 0
              ? matchNote(match.sections)
              : section.note;
          // Exactly one row is in the tab order. While a query is active the
          // selected section can be filtered out, and then the first surviving
          // row takes the Tab stop — otherwise the rail would drop out of the
          // tab order entirely.
          const inTabOrder = selected || (activeHidden && visible[0]?.id === section.id);
          return (
            <button
              key={section.id}
              id={`settings-nav-tab-${section.id}`}
              type="button"
              role="tab"
              class={`settings-nav-item${selected ? ' is-active' : ''}`}
              aria-selected={selected}
              aria-controls={`settings-panel-${section.id}`}
              // Named by its label, not by the note beside it, so "Appearance"
              // is the whole accessible name and the note arrives as a
              // description rather than as part of the name.
              aria-labelledby={`settings-nav-label-${section.id}`}
              aria-describedby={`settings-nav-note-${section.id}`}
              tabIndex={inTabOrder ? 0 : -1}
              onClick={() => onSelect(section.id, searching)}
            >
              <span class="settings-nav-icon" aria-hidden="true">
                {section.icon}
              </span>
              <span class="settings-nav-text">
                <span class="settings-nav-label" id={`settings-nav-label-${section.id}`}>
                  {section.label}
                </span>
                <span
                  class={`settings-nav-note${
                    searching && match && match.sections.length > 0 ? ' is-match' : ''
                  }`}
                  id={`settings-nav-note-${section.id}`}
                >
                  {note}
                </span>
              </span>
            </button>
          );
        })}
      </div>

      {searching && visible.length === 0 ? (
        <div class="settings-nav-empty" role="status">
          <p>No setting matches “{query.trim()}”.</p>
          <button type="button" class="link-button" onClick={() => onQueryChange('')}>
            Clear the search
          </button>
        </div>
      ) : null}
    </nav>
  );
}
