import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { apiRequest } from '../../api/client';
import { listAgents, listSessions } from '../../api/endpoints';
import type { AgentSummary, ControlChatSummary, SessionSummary } from '../../api/types';
import * as actions from '../../state/actions';
import * as store from '../../state/store';
import {
  CONTROL_REFRESH_MS,
  controlChats,
  controlOverview,
  controlOverviewError,
  controlPlaneOpen,
  loadControlOverview,
} from '../../state/controlPlane';
import { Dialog } from '../Dialog';
/* The shell's sheet, imported before the regions below so their CSS lands after
 * it: the shared vocabulary is the base a region sheet may override, and with
 * equal specificity the later sheet wins. The regions import their own sheets, so
 * this is the one place the order is decided. */
import '../../styles/control-shared.css';
import { ControlRail } from './ControlRail';
import { ControlOverview, modelChoices } from './ControlOverview';
import { ControlSession } from './ControlSession';
import { type Filter, chatTitle } from './controlModel';

/**
 * The control plane: every project in one browser, and one place to act on it.
 *
 * This file is the shell. It owns the state and the effects - what is selected,
 * what the last read said, what the launch box holds - and composes three regions
 * by props: the rail (`ControlRail`), the overview (`ControlOverview`) and the open
 * conversation (`ControlSession`). A region never reads from the store something
 * this shell already read; it is handed the value instead.
 *
 * The overview is one snapshot (`GET /api/control/overview` via
 * `state/controlPlane.ts`), never five derived lists, so the counters and the
 * lists beneath them cannot disagree with each other. A failed read marks the
 * snapshot stale and keeps it on screen, because "nothing is running anywhere"
 * is a different and wrong answer.
 */
export function ControlPlane(): JSX.Element {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [listError, setListError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>('all');
  const [query, setQuery] = useState('');
  const [draft, setDraft] = useState('');
  const [launchProject, setLaunchProject] = useState('');
  const [launchModelKey, setLaunchModelKey] = useState('');
  const [launchEffort, setLaunchEffort] = useState(() => store.modelSelection.value.effort ?? 'default');
  const [renaming, setRenaming] = useState<SessionSummary | null>(null);
  const [renameDraft, setRenameDraft] = useState('');
  const [deleting, setDeleting] = useState<SessionSummary | null>(null);
  /**
   * Every subagent of the open session, whatever its status.
   *
   * The snapshot's `live_agents` only carries the running ones - that is the
   * right list for "who is working right now", and the wrong one for "what did
   * this chat delegate". A finished child is still the answer to that question, so
   * the open conversation reads its own session's agents from
   * `GET /api/agents?session=`, which lists every row the store has.
   */
  const [detailAgents, setDetailAgents] = useState<AgentSummary[] | null>(null);
  const generation = useRef(0);
  const navigation = useRef<Promise<void>>(Promise.resolve());
  const mounted = useRef(true);
  const refreshTick = useRef(0);
  /** The rail region, focused when the plane opens (see the mount effect). */
  const browserRef = useRef<HTMLElement>(null);

  const overview = controlOverview.value;
  const staleReason = controlOverviewError.value;
  const chats = controlChats();
  /** Read once so the three regions are handed the list instead of reading it. */
  const projects = store.projects.value;

  /**
   * Read the rail and the snapshot together.
   *
   * One timer rather than `startControlPlaneRefresh()` plus a second one: the
   * rail also needs `/api/sessions` (the snapshot only carries the running,
   * attention and recent slices), so both reads share the snapshot's period
   * instead of polling the same server twice on two schedules.
   */
  const refresh = async (): Promise<void> => {
    const token = ++refreshTick.current;
    const [sessionResult] = await Promise.allSettled([listSessions(), loadControlOverview()]);
    if (!mounted.current || token !== refreshTick.current) return;
    if (sessionResult.status === 'fulfilled') {
      setSessions(sessionResult.value);
      setListError(null);
    } else {
      setListError(
        sessionResult.reason instanceof Error ? sessionResult.reason.message : 'Could not load sessions.',
      );
    }
    setLoading(false);
  };

  useEffect(() => {
    if (launchProject === '' || !store.projects.value.some((project) => project.id === launchProject)) {
      setLaunchProject(store.currentProjectId.value ?? store.projects.value[0]?.id ?? '');
    }
  }, [store.projects.value, launchProject]);

  // The launch box starts on the model a run would use right now, and follows
  // the config when it arrives (`/api/config` is read after this mounts).
  useEffect(() => {
    if (launchModelKey !== '') return;
    const selection = store.modelSelection.value;
    const choice = modelChoices(store.config.value).find(
      (item) => item.providerId === selection.providerId && item.modelId === selection.modelId,
    );
    if (choice) setLaunchModelKey(choice.key);
  }, [store.config.value, launchModelKey]);

  useEffect(() => {
    mounted.current = true;
    // Land the reader inside the plane: it replaced the workspace on screen, so
    // focus must not stay on the button that opened it (which is now gone) or
    // fall back to `<body>`. The browser's own focus fix-up for an unmounted
    // trigger runs *after* this effect and can undo a single `focus()` call, so
    // it is retried until something inside the plane actually holds focus.
    // `setTimeout`, not `requestAnimationFrame`: rAF is throttled (or paused) in
    // a backgrounded/headless tab, which is exactly when a retry is still needed.
    let attempts = 0;
    let retry = 0;
    const focusRail = (): void => {
      const rail = browserRef.current;
      if (!rail) return;
      rail.focus({ preventScroll: true });
      const inside = rail.contains(document.activeElement) || document.activeElement === rail;
      attempts += 1;
      // Bounded: a genuinely un-focusable plane must not retry forever.
      if (!inside && attempts < 12) retry = window.setTimeout(focusRail, 16);
    };
    focusRail();
    void refresh();
    const timer = window.setInterval(() => void refresh(), CONTROL_REFRESH_MS);
    return () => {
      mounted.current = false;
      generation.current++;
      window.clearTimeout(retry);
      window.clearInterval(timer);
    };
  }, []);

  /**
   * The open session's own subagents, on their own read.
   *
   * Keyed on the selection: opening another chat must not show the previous
   * chat's children, and a failed read must leave the rows already on screen
   * alone rather than blanking them (the snapshot's live agents stay the
   * fallback). A control chat has no project, so the read is by session id - the
   * only scope that finds them.
   */
  useEffect(() => {
    if (!selected) {
      setDetailAgents(null);
      return;
    }
    // Drop the previous chat's rows before the new read: `Subagents` falls back
    // to the snapshot meanwhile, which is the honest answer for *this* chat.
    setDetailAgents(null);
    let cancelled = false;
    const read = async (): Promise<void> => {
      try {
        const agents = await listAgents({ session: selected });
        if (!cancelled) setDetailAgents(agents);
      } catch {
        // Keep what is on screen; the snapshot still answers "who is live".
      }
    };
    void read();
    const timer = window.setInterval(() => void read(), CONTROL_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [selected]);

  const projectName = (id: string | null | undefined): string =>
    id ? projects.find((project) => project.id === id)?.name ?? id : 'No project';

  /**
   * Open one session in the ordinary workspace: switch its project first, because
   * the chat, composer and tools all read the active project.
   */
  const navigate = (session: { id: string; project_id: string | null }, close?: boolean): Promise<void> => {
    const token = ++generation.current;
    setSelected(session.id); setOpening(true); setError(null);
    navigation.current = navigation.current.catch(() => {}).then(async () => {
      if (!mounted.current || token !== generation.current) return;
      if (session.project_id) {
        if (!store.projects.value.some((project) => project.id === session.project_id)) {
          throw new Error('This session’s project is unavailable.');
        }
        if (store.currentProjectId.value !== session.project_id) await actions.switchProject(session.project_id);
        if (!mounted.current || token !== generation.current) return;
        if (store.currentProjectId.value !== session.project_id) throw new Error('Could not switch project.');
      }
      if (store.currentSessionId.value !== session.id && !await actions.openSession(session.id)) {
        throw new Error('Could not open session.');
      }
      if (!mounted.current || token !== generation.current) return;
      setOpening(false);
      if (close) controlPlaneOpen.value = false;
    }).catch((e: unknown) => {
      if (mounted.current && token === generation.current) {
        setOpening(false); setError(e instanceof Error ? e.message : 'Could not open session.');
      }
    });
    return navigation.current;
  };

  /** Open a control chat: the newest one, or a brand-new one. */
  const openControlSession = async (createNew: boolean): Promise<string | null> => {
    const token = ++generation.current;
    setOpening(true); setError(null);
    try {
      const newest: ControlChatSummary | null = chats[0] ?? null;
      if (createNew || !newest) {
        const created = await apiRequest<{ session_id: string; project_id: string | null }>(
          '/api/control/sessions', { method: 'POST', body: {} },
        );
        if (!mounted.current || token !== generation.current) return null;
        // The snapshot is the only list the rail draws, so re-read it before
        // opening the chat: otherwise the rail and the header disagree for a
        // whole poll interval about whether the chat exists.
        await refresh();
        await navigate({ id: created.session_id, project_id: created.project_id ?? null });
        return created.session_id;
      }
      await navigate({ id: newest.id, project_id: newest.project_id ?? null });
      return newest.id;
    } catch (e) {
      if (mounted.current && token === generation.current) {
        setOpening(false); setError(e instanceof Error ? e.message : 'Could not open control chat.');
      }
      return null;
    }
  };

  const launch = async (event: JSX.TargetedSubmitEvent<HTMLFormElement>): Promise<void> => {
    event.preventDefault();
    const text = draft.trim();
    const target = store.projects.value.find((project) => project.id === launchProject);
    const instruction = target
      ? `Start work in project "${target.name}" (project_id: ${target.id}).\n\n${text}`
      : text;
    if (text === '') { await openControlSession(false); return; }
    const submitted = draft;
    const id = await openControlSession(false);
    // Only fill the composer once the control session is really the open one.
    // On failure, leave the launch prompt exactly where it can be retried. On
    // success, clear only that submission: edits made while it was pending are
    // new work and must not be replaced by an older continuation.
    if (id && store.currentSessionId.value === id) {
      // Pin the chosen model before the prompt lands, so the first run of the
      // chat uses the model the launch box showed. A refused pin must not
      // swallow the prompt: the draft is handed over either way.
      const choice = modelChoices(store.config.value).find((item) => item.key === launchModelKey);
      if (choice) {
        await actions.setSessionModelSelection(
          choice.providerId,
          choice.modelId,
          launchEffort === 'default' ? null : launchEffort,
        );
      }
      actions.fillComposer(instruction);
      setDraft((current) => (current === submitted ? '' : current));
    }
  };

  /**
   * Rename through the shared action, then re-read the snapshot the rail draws.
   *
   * The action patches `store.sessions.value` as well, which is what keeps a
   * project session's row in the workspace rail correct without waiting for an
   * SSE frame - the control plane draws its own list, so that does not show.
   */
  const rename = async (session: SessionSummary, title: string): Promise<void> => {
    const trimmed = title.trim();
    if (trimmed === '') return;
    setRenaming(null);
    const ok = await actions.renameSessionById(session.id, trimmed);
    if (!ok) return;
    await refresh();
  };

  /** Delete one session; the open one returns the pane to the overview. */
  const remove = async (session: SessionSummary): Promise<void> => {
    setDeleting(null);
    try {
      await apiRequest(`/api/sessions/${encodeURIComponent(session.id)}`, { method: 'DELETE' });
      if (selected === session.id) { setSelected(null); setError(null); }
      // The workspace store may still be pointing at the row that just vanished;
      // leaving it there would make the ordinary workspace open a dead session
      // after **Project workspace**. `clearSelection` is the full teardown (id,
      // transcript, remembered pin, last-session id), not a bare assignment.
      if (store.currentSessionId.value === session.id) actions.closeSession();
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not delete this session.');
    }
  };

  const chosen = sessions.find((item) => item.id === selected)
    ?? chats.find((item) => item.id === selected)
    ?? null;
  const isControl = chats.some((item) => item.id === selected);

  /** Rename and delete, as the row menus invoke them; both open a dialog here. */
  const rowActions = {
    onRename: (session: SessionSummary) => { setRenameDraft(chatTitle(session)); setRenaming(session); },
    onDelete: (session: SessionSummary) => setDeleting(session),
  };

  const openInProject = (): void => {
    if (!selected) return;
    const target = chosen ?? store.activeSession.value;
    if (!target) return;
    navigate({ id: target.id, project_id: target.project_id }, true);
  };

  const back = (): void => { setSelected(null); setError(null); };

  return <div class={`control-plane cp${selected ? ' detail-open' : ''}`}>
    {/* `tabIndex={-1}` + the mount effect: opening the plane used to leave focus
        in the app's project rail, so the next nine tabs walked the rail behind
        this screen before reaching it. Focusing the region itself puts the
        reader (and the ring) inside the plane, and the first Tab then lands on
        its own refresh button. */}
    <ControlRail
      railRef={browserRef}
      chats={chats}
      sessions={sessions}
      loading={loading}
      listError={listError}
      selected={selected}
      filter={filter}
      onFilter={setFilter}
      query={query}
      onQuery={setQuery}
      projectCount={projects.length}
      projectName={projectName}
      onRefresh={() => void refresh()}
      onNewChat={() => void openControlSession(true)}
      onNavigate={navigate}
      {...rowActions}
    />

    <main class="cp-pane">
      {listError ? <div class="cp-error" role="alert">Could not refresh sessions: {listError}. The list may be stale.</div> : null}
      {error ? <div class="cp-error" role="alert">{error} <button type="button" onClick={() => setError(null)}>Dismiss</button></div> : null}
      {/* A failed read never clears the snapshot: it is marked stale instead. */}
      {staleReason ? <p class="cp-stale" role="status">stale · {staleReason}</p> : null}
      {selected ? <ControlSession
        selected={selected}
        chosen={chosen}
        control={isControl}
        opening={opening}
        ready={store.currentSessionId.value === selected}
        detailAgents={detailAgents}
        claimedTotal={chats.find((chat) => chat.id === selected)?.agents.total ?? null}
        projects={projects}
        projectName={projectName}
        onBack={back}
        onOpenInProject={openInProject}
        onNavigate={navigate}
      /> : <ControlOverview
        overview={overview}
        chatCount={chats.length}
        projects={projects}
        draft={draft}
        onDraft={setDraft}
        launchProject={launchProject}
        onLaunchProject={setLaunchProject}
        launchModelKey={launchModelKey}
        onLaunchModel={setLaunchModelKey}
        launchEffort={launchEffort}
        onLaunchEffort={setLaunchEffort}
        opening={opening}
        onLaunch={(event) => void launch(event)}
        projectName={projectName}
        onNavigate={navigate}
        onFilterProject={(name) => { setFilter('all'); setQuery(name); }}
      />}
    </main>

    <Dialog open={renaming !== null} title="Rename session" onClose={() => setRenaming(null)} class="dialog-confirm">
      {renaming ? <form
        class="confirm-body"
        onSubmit={(event) => { event.preventDefault(); void rename(renaming, renameDraft); }}
      >
        <label class="field-label" for="cp-rename-input">Title</label>
        <input
          id="cp-rename-input"
          class="input"
          value={renameDraft}
          aria-label="Session title"
          onInput={(event) => setRenameDraft(event.currentTarget.value)}
        />
        <div class="dialog-actions">
          <button type="button" class="button" onClick={() => setRenaming(null)}>Cancel</button>
          <button type="submit" class="button button-primary">Rename</button>
        </div>
      </form> : null}
    </Dialog>

    <Dialog open={deleting !== null} title="Delete session?" onClose={() => setDeleting(null)} class="dialog-confirm">
      {deleting ? <div class="confirm-body">
        <p>
          Delete <strong>{chatTitle(deleting)}</strong>? Its messages, runs and approvals are removed.
          This cannot be undone.
        </p>
        <div class="dialog-actions">
          <button type="button" class="button" data-autofocus onClick={() => setDeleting(null)}>Cancel</button>
          <button type="button" class="button button-danger" onClick={() => void remove(deleting)}>
            Delete session
          </button>
        </div>
      </div> : null}
    </Dialog>
  </div>;
}
