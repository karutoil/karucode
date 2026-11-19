import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import type { JSX } from 'preact';
import { useFocusTrap } from '../lib/dom';
import { modKey } from '../lib/platform';
import * as actions from '../state/actions';
import { controlPlaneOpen } from '../state/controlPlane';
import * as railPeek from '../state/railPeek';
import { invalidateFacts, loadFactsFor } from '../state/sessionFacts';
import * as store from '../state/store';
import { PlusIcon, SearchIcon } from './icons';
import { Button, Input, Keycap } from '../ui';
import {
  passesFilter,
  PROJECTS_HOVER_CLOSE_MS,
  PROJECTS_HOVER_OPEN_MS,
  SESSION_FILTERS,
  sessionGroup,
  type SessionFilter,
} from '../features/rail/helpers';
import { ProjectSwitcher } from '../features/rail/ProjectDropdown';
import { RailFooter } from '../features/rail/RailFooter';
import { RailPeek } from '../features/rail/RailPeek';
import { SessionList, type SessionListProps } from '../features/rail/SessionList';

export {
  passesFilter,
  PROJECTS_HOVER_CLOSE_MS,
  PROJECTS_HOVER_OPEN_MS,
  RailPeek,
  SESSION_FILTERS,
  sessionGroup,
  SessionList,
  type SessionFilter,
  type SessionListProps,
};

export function SessionsRail(): JSX.Element {
  const sessions = store.sessions.value;
  const currentId = store.currentSessionId.value;
  const isSheetOpen = store.mobileSheet.value === 'sessions';
  const isNarrow = store.narrowViewport.value;
  const project = store.currentProject.value;
  const railRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState('');
  const [sessionFilter, setSessionFilter] = useState<SessionFilter>('all');
  useFocusTrap(railRef, isNarrow && isSheetOpen, actions.closeSheets);
  const unread = store.unread.value;

  const filterCounts = useMemo<Record<SessionFilter, number>>(() => {
    const counts: Record<SessionFilter, number> = { all: sessions.length, needs: 0, running: 0, changes: 0 };
    for (const session of sessions) {
      if (passesFilter(session, 'needs', unread[session.id] ?? 0)) counts.needs += 1;
      if (session.running) counts.running += 1;
    }
    return counts;
  }, [sessions, unread]);

  // A filter whose count drops to zero (the last run finished) would strand the
  // reader on an empty list with no obvious way back.
  useEffect(() => {
    if (sessionFilter !== 'all' && filterCounts[sessionFilter] === 0) setSessionFilter('all');
  }, [sessionFilter, filterCounts]);

  // The per-session fingerprints under each title. Bounded and coalesced by the
  // loader, and keyed by the newest-first list, so a long rail reads the newest
  // rows first and never more than the cap.
  const sessionKey = sessions.map((session) => session.id).join(',');
  const projectId = project?.id ?? null;
  const previousRunning = useRef<Record<string, boolean>>({});
  // A project switch must not let a late response from the previous project
  // paint the new rail. The loader also guards the response, but clearing here
  // removes old fingerprints immediately.
  useEffect(() => {
    invalidateFacts();
  }, [projectId]);
  useEffect(() => {
    const finished = sessions
      .filter((session) => previousRunning.current[session.id] === true && !session.running)
      .map((session) => session.id);
    if (finished.length > 0) invalidateFacts(finished);
    previousRunning.current = Object.fromEntries(sessions.map((session) => [session.id, session.running]));
  }, [sessionKey, sessions]);
  // The loader owns TTL, concurrency and request coalescing. This effect only
  // changes when the list identity changes, so status updates do not refetch.
  useEffect(() => {
    if (sessionKey === '') return;
    void loadFactsFor(sessionKey.split(','));
  }, [sessionKey, projectId]);

  // `/` focuses the session search, like the mock's keycap promises.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== '/' || event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target;
      if (
        target instanceof HTMLElement &&
        target.closest('input, textarea, select, [contenteditable="true"]')
      ) {
        return;
      }
      if (store.settingsOpen.value || store.paletteOpen.value || store.themeStudioOpen.value) return;
      event.preventDefault();
      if (isNarrow && store.mobileSheet.value !== 'sessions') actions.toggleRail();
      else if (!isNarrow && store.railCollapsed.value) actions.toggleRail();
      window.setTimeout(() => searchRef.current?.focus(), 0);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [isNarrow]);

  // The collapsed rail's fly-out replaces the rail rather than sitting beside it, so
  // the page never holds two session lists (and two "Sessions" landmarks) at once.
  const peekId = railPeek.peekProjectId.value;
  const peekActive = peekId !== null && !isNarrow && store.railCollapsed.value && !controlPlaneOpen.value;
  if (peekActive && peekId !== null) return <RailPeek projectId={peekId} />;

  return (
    <div
      ref={railRef}
      class={`rail${isSheetOpen ? ' is-open' : ''}`}
      aria-label="Sessions"
      // A plain div has no implicit role, so the rail's landmark (complementary,
      // what an <aside> would have been) and the phone sheet's dialog are both
      // explicit. preact 11's per-element ARIA unions do not allow dialog on an
      // aside, so the element is a div.
      role={isSheetOpen ? 'dialog' : 'complementary'}
      aria-modal={isSheetOpen ? 'true' : undefined}
    >
      <div class="rail-inner">
        {/* One switcher owns project navigation and management in both layouts.
         * Wide screens get a compact disclosure; the phone sheet keeps the full
         * horizontal list and its existing close/control-plane actions. */}
        {isNarrow ? (
          <>
            <ProjectSwitcher />
            <Button
              variant="secondary"
              class="new-session h-[31px] px-4 rounded-md bg-surface text-text-secondary border-line-strong"
              testId="phone-control-plane"
              onClick={() => {
                actions.closeSheets();
                controlPlaneOpen.value = true;
              }}
            >
              Control plane
            </Button>
          </>
        ) : null}
        {isNarrow ? null : (
          <div class="side-head">
            <ProjectSwitcher desktop />
          </div>
        )}

        <Button
          variant="secondary"
          class="new-session h-[31px] px-4 rounded-md bg-surface text-text-secondary border-line-strong"
          aria-label={`New session (${modKey()} + N)`}
          testId="new-session"
          onClick={() => actions.startDraftSession()}
        >
          <span class="new-left">
            <PlusIcon size={15} /> New session
          </span>
          <span class="shortcut">{modKey()} N</span>
        </Button>

        <label class="side-search">
          <SearchIcon size={15} />
          {/* `data-escape-local`: Escape first clears the query (and a second
            * press closes the sheet on a phone). Without the marker the sheet's
            * capture-phase focus trap would close everything on the first press. */}
          <Input
            inputRef={searchRef}
            type="search"
            size="sm"
            class="border-0 bg-transparent px-0 h-full text-label"
            value={query}
            placeholder="Find a session…"
            aria-label="Find a session"
            autoComplete="off"
            spellCheck={false}
            data-escape-local
            testId="session-search"
            onInput={(event) => setQuery(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                if (query !== '') {
                  event.preventDefault();
                  setQuery('');
                } else if (isNarrow) {
                  // Nothing left to clear: the next press is the sheet's.
                  event.preventDefault();
                  actions.closeSheets();
                }
              } else if (event.key === 'Enter') {
                const first = sessions.find((session) =>
                  passesFilter(session, sessionFilter, unread[session.id] ?? 0) &&
                  (session.title || '').toLowerCase().includes(query.trim().toLowerCase()),
                );
                if (first) {
                  event.preventDefault();
                  actions.switchSession(first.id);
                }
              }
            }}
          />
          <Keycap class="keycap">/</Keycap>
        </label>

        {/* The mock's `.chips`: one-tap filters over the list, each with its own
         * count, and a chip that would filter to nothing is not offered. */}
        <div class="session-chips" role="group" aria-label="Filter sessions">
          {SESSION_FILTERS.map((filter) => {
            const count = filterCounts[filter.id] ?? 0;
            if (filter.id !== 'all' && count === 0) return null;
            return (
              <Button
                key={filter.id}
                variant="ghost"
                size="sm"
                class={`filter-chip h-[24px] px-3 gap-2 text-micro${
                  sessionFilter === filter.id ? ' is-on' : ''
                }${filter.id === 'needs' ? ' is-alert' : ''}`}
                aria-pressed={sessionFilter === filter.id}
                title={filter.title}
                testId={`session-filter-${filter.id}`}
                onClick={() => setSessionFilter(filter.id)}
              >
                {filter.label}
                <span class="filter-chip-count">{count}</span>
              </Button>
            );
          })}
        </div>

        <nav class="session-nav" aria-label="Session list">
          <SessionList
            sessions={sessions}
            currentId={currentId}
            now={store.clock.value}
            loading={store.sessionsLoading.value}
            workspace={
              project?.path ?? store.health.value?.workspace ?? store.config.value?.workspace
            }
            query={query}
            filter={sessionFilter}
            annotations={store.sessionAnnotations.value}
            onCreate={() => actions.startDraftSession()}
            onSelect={actions.switchSession}
            onRename={actions.renameSessionById}
            onDelete={actions.deleteSessionById}
          />
        </nav>

        <RailFooter />
      </div>
    </div>
  );
}
