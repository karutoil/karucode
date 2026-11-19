import { useEffect } from 'preact/hooks';
import type { JSX } from 'preact';
import { modKey } from '../../lib/platform';
import { pluginText, UI_REPLACE_NEW_SESSION_LABEL } from '../../components/uiContributions';
import * as actions from '../../state/actions';
import * as railPeek from '../../state/railPeek';
import { loadFactsFor } from '../../state/sessionFacts';
import * as store from '../../state/store';
import { CloseIcon, FolderIcon, PanelIcon, PlusIcon } from '../../components/icons';
import { Button, IconButton } from '../../ui';
import { SessionList } from './SessionList';

/**
 * The collapsed rail's fly-out: one project's sessions, over the chat.
 *
 * `Ctrl/Cmd+\` gives the sessions rail zero width, which is right for a wide
 * conversation and a dead end when all you want is to open a session. Hovering a
 * project tile while the rail is collapsed shows this instead: the same rows, the
 * same rename and delete menus, positioned against the project rail and out of flow
 * so the conversation never resizes to make room. Hovering a *different* project
 * shows that project — the rows come from `state/railPeek.ts`, which reads the list
 * once and caches it, because the store only holds the open project's sessions.
 *
 * It closes when the pointer leaves (after a short grace period, so clipping the
 * edge on the way past does not flash it) and when a row is opened. It is a view,
 * not a mode: nothing is remembered once it is gone.
 */
export function RailPeek({ projectId }: { projectId: string }): JSX.Element {
  const projects = store.projects.value;
  const project = projects.find((candidate) => candidate.id === projectId) ?? null;
  const isCurrent = store.currentProjectId.value === projectId;
  const sessions = isCurrent
    ? store.sessions.value
    : railPeek.peekSessions.value[projectId] ?? [];
  const loading = isCurrent
    ? store.sessionsLoading.value
    : railPeek.peekLoading.value[projectId] ?? false;
  const error = isCurrent ? null : railPeek.peekError.value[projectId] ?? null;
  const initial = (project?.name ?? 'k').trim().slice(0, 1).toLowerCase() || 'k';

  // The same fingerprints the full rail shows under each title, from the same
  // bounded loader: a peek that dropped them would make the rows twice as wide and
  // half as informative as the list it replaces.
  const sessionKey = sessions.map((session) => session.id).join(',');
  useEffect(() => {
    if (sessionKey === '') return;
    void loadFactsFor(sessionKey.split(','));
  }, [sessionKey, projectId]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      // A rename or a menu inside the fly-out consumes its own Escape (they mark
      // the event `defaultPrevented`); tearing the panel down as well would cancel
      // the edit *and* close the list behind it, so the reader loses both.
      if (event.key === 'Escape' && !event.defaultPrevented) railPeek.closePeek();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  const openSession = (id: string): void => {
    railPeek.closePeek();
    if (isCurrent) actions.switchSession(id);
    else void actions.openSessionInProject(projectId, id);
  };

  const startDraft = (): void => {
    railPeek.closePeek();
    // `actions.startDraftSession()` drafts in `store.currentProjectId`, which is
    // *not* the project this preview belongs to while the fly-out covers another
    // one. Switch first so the empty state's "New session" starts a conversation
    // in the project the reader is looking at, not silently in the open one.
    void (async () => {
      if (!isCurrent) {
        await actions.switchProject(projectId);
        // A newer navigation may have completed while this switch was pending.
        // Never let the stale continuation draft in that newer project.
        if (store.currentProjectId.value !== projectId) return;
      }
      actions.startDraftSession();
    })();
  };

  // Renames and deletes are by session id, so they work for any project; the cached
  // list has to be re-read because it is not the store's current-project list.
  const refreshPeek = (): void => {
    if (isCurrent) return;
    railPeek.invalidatePeekSessions(projectId);
    void railPeek.loadPeekSessions(projectId, true);
  };

  return (
    <div
      class="rail rail-peek"
      aria-label={`Sessions in ${project?.name ?? 'this project'}`}
      data-project-id={projectId}
      onMouseEnter={railPeek.holdPeek}
      onMouseLeave={() => railPeek.schedulePeekClose()}
    >
      <div class="rail-inner">
        <div class="side-head">
          <div class="project-heading">
            <span class="project-title-glyph" aria-hidden="true">
              {initial}
            </span>
            <h2 class="project-title" title={project?.path ?? 'Unknown project'}>
              <span class="project-title-text">{project?.name ?? 'Unknown project'}</span>
              <span class="path-hint">
                {isCurrent ? project?.path ?? '' : `${sessions.length} ${sessions.length === 1 ? 'session' : 'sessions'}`}
              </span>
            </h2>
            <div class="side-head-actions">
              <IconButton
                size="md"
                class="side-mini-btn w-[28px] h-[28px]"
                label="Pin the session list open"
                aria-keyshortcuts="Control+B Meta+B"
                title={`Pin the session list open — ${modKey()} B`}
                icon={<PanelIcon size={15} />}
                testId="peek-pin"
                onClick={() => {
                  railPeek.closePeek();
                  // The rail is collapsed by definition while peeking, so this
                  // toggle is what shows it for real.
                  actions.toggleRail();
                }}
              />
              <IconButton
                size="md"
                class="side-mini-btn w-[28px] h-[28px]"
                label="Close the session preview"
                title="Close (Esc)"
                icon={<CloseIcon size={15} />}
                testId="peek-close"
                onClick={railPeek.closePeek}
              />
            </div>
          </div>
        </div>

        {isCurrent ? (
          <Button
            variant="secondary"
            class="new-session h-[31px] px-4 rounded-md bg-surface text-text-secondary border-line-strong"
            onClick={startDraft}
            aria-label={`New session (${modKey()} + N)`}
            testId="peek-new-session"
          >
            <span class="new-left">
              <PlusIcon size={15} />{' '}
              {pluginText(store.pluginUi.value, UI_REPLACE_NEW_SESSION_LABEL, 'New session')}
            </span>
            <span class="shortcut">{modKey()} N</span>
          </Button>
        ) : (
          <Button
            variant="secondary"
            class="new-session h-[31px] px-4 rounded-md bg-surface text-text-secondary border-line-strong"
            aria-label={`Open ${project?.name ?? 'this project'}`}
            testId="peek-open-project"
            onClick={() => {
              railPeek.closePeek();
              void actions.switchProject(projectId);
            }}
          >
            <span class="new-left">
              <FolderIcon size={15} /> Open this project
            </span>
          </Button>
        )}

        <nav class="session-nav" aria-label={`Sessions in ${project?.name ?? 'this project'}`}>
          {error ? (
            <div class="rail-empty" role="status">
              <p class="rail-empty-title">Could not read those sessions</p>
              <p class="rail-empty-body">{error}</p>
              <Button
                testId="peek-retry"
                onClick={() => void railPeek.loadPeekSessions(projectId, true)}
              >
                Try again
              </Button>
            </div>
          ) : (
            <SessionList
              sessions={sessions}
              currentId={isCurrent ? store.currentSessionId.value : null}
              now={store.clock.value}
              loading={loading}
              workspace={project?.path}
              annotations={
                isCurrent
                  ? store.sessionAnnotations.value
                  : railPeek.peekAnnotations.value[projectId] ?? []
              }
              onCreate={startDraft}
              onSelect={openSession}
              onRename={async (id, title) => {
                const ok = await actions.renameSessionById(id, title);
                if (ok) refreshPeek();
                return ok;
              }}
              onDelete={async (id) => {
                const ok = await actions.deleteSessionById(id);
                if (ok) refreshPeek();
                return ok;
              }}
            />
          )}
        </nav>
      </div>
    </div>
  );
}
