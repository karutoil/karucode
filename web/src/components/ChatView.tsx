import type { JSX } from 'preact';
import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import type { ChatItem } from '../state/chat';
import { copyText } from '../lib/clipboard';
import { useHeightVariable } from '../lib/dockHeight';
import { fileName } from '../lib/fileKind';
import { useHistoryLoader } from '../lib/historyLoader';
import { modKey } from '../lib/platform';
import { useStickyBottom } from '../lib/stickyBottom';
import type { TreeLoader } from '../lib/mentions';
import { getGitStatus } from '../api/endpoints';
import * as actions from '../state/actions';
import * as terminalState from '../state/terminals';
import { contextUsage, formatContextPercent, totalUsage } from '../state/selectors';
import * as store from '../state/store';
import * as workbench from '../state/workbench';
import * as workspaceTabs from '../state/workspaceTabs';
import { GitManager } from './git/GitManager';
import { ChatTranscript } from './ChatTranscript';
import { Composer } from './Composer';
import { EmptyState } from './EmptyState';
import { ErrorBoundary } from './ErrorBoundary';
import { FileTabView } from './FileViewer';
import { Inspector } from './Inspector';
import { ResultResizer } from './ResultResizer';
import { ContextDrawer } from './ContextDrawer';
import { WorkbenchTaskMenu } from './WorkbenchTaskMenu';
import { LiveAnnouncer } from './LiveAnnouncer';
import { PluginSlot } from './PluginSlot';
import { UI_SLOT_SESSION_HEADER } from './uiContributions';
import { Tabs, type TabItem } from './Tabs';
import { TerminalTab } from './TerminalTab';
import {
  AlertIcon,
  BellIcon,
  BranchIcon,
  ChatIcon,
  ChevronDownIcon,
  FileCodeIcon,
  GitIcon,
  MenuIcon,
  RefreshIcon,
  SearchIcon,
  TerminalIcon,
} from './icons';
/**
 * The centre-pane views of the workspace (the mock's workspace tabs), plus one
 * tab per open file.
 *
 * `terminal` is the odd one out: it is a real shell, not a view over the
 * session, so it works with no session open and its state belongs to the server
 * rather than to the conversation. A `file:` view carries its workspace path in
 * the id, so one strip can hold any number of open documents
 * (`state/workspaceTabs.ts` owns the list and the selection).
 */
export type WorkspaceView = workspaceTabs.WorkspaceView;

/**
 * Plain-text transcript for the "Copy" header action: speaker, message text,
 * tool calls and command output, in the order they happened.
 */
export function transcriptToText(items: readonly ChatItem[], title: string): string {
  const lines = [`# ${title}`, ''];
  for (const item of items) {
    if (item.kind === 'message') {
      const who = item.role === 'user' ? 'You' : item.role === 'assistant' ? 'karucode' : item.role;
      if (item.content.trim() === '') continue;
      lines.push(`**${who}**`, '', item.content.trim(), '');
    } else if (item.kind === 'tool') {
      lines.push(`\`${item.name}\` → ${item.status}`, '');
      if (item.output.trim() !== '') lines.push('```', item.output.trim(), '```', '');
    } else if (item.kind === 'command') {
      lines.push('```', `$ ${item.command}`, item.stdout.trim(), item.stderr.trim(), '```', '');
    } else if (item.kind === 'approval') {
      lines.push(`> Approval: ${item.approval.title} — ${item.approval.status}`, '');
    }
  }
  return lines.join('\n').replace(/\n{3,}/g, '\n\n').trimEnd();
}

/**
 * The current branch, as the mock's top-right chip.
 *
 * The mock always shows a branch because it is always in a repo; ours shows the
 * chip only when the server reports a real branch, so a non-git workspace gets
 * no fabricated state. Clicking it opens the inspector's Changes tab, which is
 * where the diff for that branch lives.
 */
function BranchChip({ onShowChanges }: { onShowChanges?: () => void }): JSX.Element | null {
  const projectId = store.currentProjectId.value;
  const [branch, setBranch] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    setBranch(null);
    getGitStatus(projectId, { signal: controller.signal })
      .then((status) => {
        if (controller.signal.aborted) return;
        setBranch(status.is_repo ? status.branch : null);
      })
      .catch(() => {
        // No chip when git is unavailable; the source-control manager owns the loud
        // error.
      });
    return () => controller.abort();
  }, [projectId]);

  if (!branch) return null;
  // The branch name is the accessible name: on a phone the visible text becomes
  // the icon alone, and a button with no text would have no name there.
  return (
    <button
      type="button"
      class="head-btn branch-chip"
      aria-label={`Branch ${branch} — open Changes`}
      title={`On branch ${branch} — open Changes`}
      onClick={onShowChanges}
    >
      <BranchIcon size={13} />
      <span class="branch-chip-name" title={branch}>
        {branch}
      </span>
    </button>
  );
}

function ProviderBanner(): JSX.Element | null {
  if (store.providerConfigured.value) return null;
  return (
    <div class="banner banner-warn" role="status">
      <AlertIcon size={15} />
      <div class="banner-body">
        <strong>No model provider configured.</strong> Set a base URL and model before sending a message.
      </div>
      <button type="button" class="button button-small" onClick={actions.openSettings}>
        Open settings
      </button>
    </div>
  );
}

function ConnectionBanner(): JSX.Element | null {
  const connection = store.connection.value;
  const offline = connection.api === 'offline';
  const reconnecting = connection.stream === 'reconnecting' || connection.stream === 'blocked';
  if (!offline && !reconnecting) return null;
  const blocked = connection.stream === 'blocked';
  return (
    <div class={`banner ${offline || blocked ? 'banner-danger' : 'banner-warn'}`} role="status">
      <AlertIcon size={15} />
      <div class="banner-body">
        {offline ? (
          <>
            <strong>Disconnected from karucode.</strong> Showing the last known state; retrying in the
            background.
          </>
        ) : blocked ? (
          <>
            <strong>This origin is not allowed.</strong> The server refused the event stream; add this
            host to <code>KARUCODE_ALLOWED_HOSTS</code> and restart it.
          </>
        ) : (
          <>
            <strong>Live updates paused.</strong> Reconnecting to the event stream; the transcript will resync.
          </>
        )}
        {connection.lastError ? <span class="banner-detail">{connection.lastError}</span> : null}
      </div>
      <button type="button" class="button button-small" onClick={() => void actions.retryConnection()}>
        <RefreshIcon size={13} /> Retry
      </button>
    </div>
  );
}

function RunErrorBanner(): JSX.Element | null {
  const error = store.chat.value.runError;
  if (!error) return null;
  return (
    <div class="run-error-banner" role="alert">
      <div class="run-error-head">
        <span class="run-error-kicker">
          <AlertIcon size={14} /> Run failed
        </span>
        <button type="button" class="link-button" onClick={actions.dismissRunError}>
          Dismiss
        </button>
      </div>
      <p class="run-error-message">{error.message}</p>
      {error.kind ? <p class="run-error-kind">kind: {error.kind}</p> : null}
    </div>
  );
}

function ChatSkeleton(): JSX.Element {
  return (
    <div class="chat-skeleton" aria-busy="true" aria-label="Loading conversation">
      <div class="skeleton skeleton-turn" />
      <div class="skeleton skeleton-turn skeleton-short" />
      <div class="skeleton skeleton-turn" />
      <div class="skeleton skeleton-turn skeleton-short" />
    </div>
  );
}

function SessionHeader({
  view,
  onShowChanges,
  onSelectView,
  controlPlane,
}: {
  view: WorkspaceView;
  onShowChanges: () => void;
  onSelectView: (next: WorkspaceView) => void;
  controlPlane: boolean;
}): JSX.Element {
  const session = store.activeSession.value;
  const drafting = store.draftSession.value !== null;
  const activity = store.activity.value;
  const narrow = store.narrowViewport.value;
  const project = store.currentProject.value;
  const projectName = project?.name ?? 'karucode';
  /** The same derived count the tab title carries; see `store.unreadTotal`. */
  const unreadNotifications = store.unreadTotal.value;
  const railExpanded = narrow ? store.mobileSheet.value === 'sessions' : !store.railCollapsed.value;
  const contextExpanded = workbench.contextOpen.value;
  const usage = contextUsage(store.chat.value, store.activeContextWindow.value);

  // A file tab is the document the reader is looking at, so the breadcrumb names
  // the file rather than the session behind it.
  const openFilePath = workspaceTabs.fileViewPath(view);
  const title =
    openFilePath !== null
      ? fileName(openFilePath)
      : view === 'changes'
        ? 'Source control'
        : view === 'terminal'
          ? 'Terminal'
          : session
            ? session.title || 'Untitled session'
            : drafting
              ? 'New session'
              : 'karucode';
  const activityWord = activity.kind === 'idle' ? 'ready' : activity.kind;

  const copyConversation = async (): Promise<void> => {
    const text = transcriptToText(store.chat.value.items, title);
    if (text.trim() === '') {
      store.pushToast('Nothing to copy yet.', 'info');
      return;
    }
    const copied = await copyText(text);
    store.pushToast(copied ? 'Conversation copied as Markdown.' : 'Could not copy the conversation.', copied ? 'success' : 'error');
  };

  return (
    <header class="main-head">
      <button
        type="button"
        class="head-btn head-icon sessions-toggle"
        onClick={actions.toggleRail}
        aria-label="Toggle session list"
        aria-expanded={railExpanded}
        title={`Sessions — ${modKey()}+B`}
      >
        <MenuIcon size={15} />
      </button>

      {/* The mock's breadcrumb is one line: the project (quiet) / the session
       * (loud), with the activity word as a capsule beside it. The old second
       * row - a 14px title under a project → sessions → activity trail - spent
       * 20px saying the same thing twice. */}
      <div class="breadcrumb">
        <strong title={project?.path ?? projectName}>{projectName}</strong>
        <span class="crumb-sep" aria-hidden="true">
          /
        </span>
        <h1 class="main-title" title={title}>
          {title}
        </h1>
        <span class={`head-activity status-${activity.kind}`}>{activityWord}</span>
      </div>

      <WorkspaceTabs view={view} onChange={onSelectView} controlPlane={controlPlane} />

      <div class="header-buttons">
        {/* Controls plugins contributed to the session header. Before the
         * built-in chips so they never push the bell or Context out of reach. */}
        <PluginSlot slot={UI_SLOT_SESSION_HEADER} />
        <BranchChip onShowChanges={onShowChanges} />
        {/* The notification centre's way in. It lives in the header rather than on
         * the project rail because it is the one place a bell is reachable in every
         * layout: the rail is collapsed by Ctrl+B on a desktop and hidden entirely
         * on a phone. The counter is the same derived unread total the tab title
         * shows, so the two can never disagree. */}
        <button
          type="button"
          class={`head-btn head-icon notifications-head${unreadNotifications > 0 ? ' has-unread' : ''}`}
          onClick={actions.toggleNotificationCenter}
          aria-label={
            unreadNotifications > 0
              ? `Notifications (${unreadNotifications} unread)`
              : 'Notifications'
          }
          aria-haspopup="dialog"
          aria-expanded={store.notificationCenterOpen.value}
          title={
            unreadNotifications > 0
              ? `${unreadNotifications} unread notification${unreadNotifications === 1 ? '' : 's'}`
              : 'Notifications'
          }
        >
          <BellIcon size={15} />
          {unreadNotifications > 0 ? (
            <span class="head-badge" aria-hidden="true">
              {unreadNotifications > 99 ? '99+' : unreadNotifications}
            </span>
          ) : null}
        </button>
        {!controlPlane ? (
          <button
            type="button"
            class="head-btn context-header-btn"
            onClick={() => workbench.openContext('now')}
            aria-label={usage ? `Session context — ${formatContextPercent(usage.fraction)} of the window used` : 'Session context'}
            aria-expanded={contextExpanded}
            aria-haspopup={narrow ? 'dialog' : undefined}
            aria-controls="workbench-context"
            title={`Session context, usage and access — ${modKey()}+J`}
          >
            {usage ? (
              <span class="context-header-meter" aria-hidden="true">
                <span style={{ width: `${Math.max(2, usage.fraction * 100)}%` }} />
              </span>
            ) : null}
            <span class="context-header-label">Context</span>
            {usage ? <span class="context-header-value">{formatContextPercent(usage.fraction)}</span> : null}
          </button>
        ) : null}
        <button
          type="button"
          class="head-btn head-icon command-head-btn"
          onClick={actions.togglePalette}
          aria-label="Command palette"
          aria-haspopup="dialog"
          aria-keyshortcuts="Control+K Meta+K"
          title={`Command palette — ${modKey()}+K`}
        >
          <SearchIcon size={15} />
        </button>
        <WorkbenchTaskMenu
          canCopy={store.chat.value.items.length > 0 && view === 'chat'}
          onCopy={copyConversation}
          controlPlane={controlPlane}
        />
      </div>
    </header>
  );
}

function WorkspaceTabs({
  view,
  onChange,
  controlPlane,
}: {
  view: WorkspaceView;
  onChange: (next: WorkspaceView) => void;
  controlPlane: boolean;
}): JSX.Element {
  const changeCount = useChangeCount();
  const items: TabItem<WorkspaceView>[] = [
    { id: 'chat', label: 'Conversation', icon: <ChatIcon size={15} /> },
    {
      id: 'changes',
      label: 'Source control',
      icon: <GitIcon size={15} />,
      count: changeCount ?? undefined,
    },
  ];
  // The terminal is only offered when the server has it enabled: hiding the tab
  // is the UI half of `KARUCODE_TERMINALS=0`.
  if (terminalState.terminalsEnabled.value) {
    items.push({
      id: 'terminal',
      label: 'Terminal',
      icon: <TerminalIcon size={15} />,
      count: terminalState.terminals.value.length || undefined,
    });
  }
  // Open files join the strip as closable tabs. They are a workspace-project
  // surface: a control chat has no project files to show, so it never lists them.
  if (!controlPlane) {
    for (const path of workspaceTabs.openFileTabs.value) {
      items.push({
        id: workspaceTabs.fileView(path),
        label: fileName(path),
        icon: <FileCodeIcon size={15} />,
        title: path,
        closable: true,
      });
    }
  }

  return (
    <Tabs
      items={items}
      active={view}
      onChange={onChange}
      onClose={(id) => {
        const path = workspaceTabs.fileViewPath(id);
        if (path !== null) workspaceTabs.requestCloseFileTab(path);
      }}
      label="Workspace views"
      idPrefix="workspace"
      variant="workspace"
    />
  );
}

/**
 * Number of files the working tree changed, for the *Changes* tab's count pill.
 * Re-probed when the project changes and after a run settles (that is when the
 * agent can have touched files). A non-repo or unreachable backend yields
 * `null`, so the tab simply has no badge instead of a fabricated zero.
 */
function useChangeCount(): number | null {
  const projectId = store.currentProjectId.value;
  const running = store.chat.value.running;
  const [count, setCount] = useState<number | null>(null);

  useEffect(() => {
    if (running) return;
    const controller = new AbortController();
    getGitStatus(projectId, { signal: controller.signal })
      .then((status) => {
        if (controller.signal.aborted) return;
        setCount(status.is_repo ? status.files.length : null);
      })
      .catch(() => {
        // The Changes panel owns the loud error; the tab just stays unbadged.
      });
    return () => controller.abort();
  }, [projectId, running]);

  return count;
}

export interface ChatViewProps {
  /**
   * Replaces the starter-prompt empty state. The control plane needs its own
   * first-run copy (start work in a project) instead of the coding starters.
   */
  emptyContent?: JSX.Element;
  /**
   * The control chat has no project files to mention, so `@` lists registered
   * projects instead. Passing this switches the mention index and its label.
   */
  mentionProjects?: { id: string; name: string }[];
  /** Control-plane chats omit project-scoped tools and result/context surfaces. */
  mode?: 'workspace' | 'control-plane';
}

/** `@` in the control chat: registered projects, not a workspace file index. */
export function projectMentionLoader(projects: { id: string; name: string }[]): TreeLoader {
  return async () => ({
    path: '',
    entries: projects.map((project) => ({
      name: project.name,
      path: project.name,
      kind: 'file',
      size: 0,
    })),
  });
}

export function ChatView({ emptyContent, mentionProjects, mode = 'workspace' }: ChatViewProps = {}): JSX.Element {
  const isWorkbench = mode === 'workspace';
  const currentId = store.currentSessionId.value;
  // A draft is a conversation with no server row yet: it renders as the open
  // conversation (composer enabled, starter prompts filling it) and is created by
  // the first message. See `startDraftSession` in `state/actions.ts`.
  const drafting = store.draftSession.value !== null;
  const chat = store.chat.value;
  // The selected view is module state (`state/workspaceTabs.ts`): the file
  // viewer's toolbar and the explorer open a file tab from outside this tree.
  // A control chat has no project workspace, so it never shows a file panel.
  const selected = workspaceTabs.workspaceView.value;
  const view: WorkspaceView =
    !isWorkbench && workspaceTabs.isFileView(selected) ? 'chat' : selected;
  const openFiles = isWorkbench ? workspaceTabs.openFileTabs.value : [];
  const setView = workspaceTabs.selectWorkspaceView;
  // Stable approval callback: an inline arrow here is a new prop identity on
  // every render, which would make the transcript's memo boundaries compare
  // unequal and re-render every row on every streamed token.
  const decideApproval = useCallback(
    (approvalId: string, decision: 'allow_once' | 'allow_session' | 'deny'): void => {
      void actions.resolveApproval(approvalId, decision);
    },
    [],
  );
  const showResult = useCallback((tab: store.InspectorTab): void => {
    setView('chat');
    workbench.closeContext();
    actions.selectInspectorTab(tab);
    if (store.narrowViewport.value) {
      if (store.mobileSheet.value !== 'inspector') actions.toggleInspector();
    } else if (store.inspectorCollapsed.value) {
      actions.toggleInspector();
    }
  }, []);
  const showChanges = useCallback((): void => {
    showResult('changes');
  }, [showResult]);
  const showConversation = useCallback((): void => {
    setView('chat');
    workbench.closeContext();
  }, []);
  // Lazy panel: Changes mounts its git probe only after the user asks for it.
  const [changesMounted, setChangesMounted] = useState(false);
  // The terminal list is needed to decide whether the tab exists at all, so it
  // is fetched on mount rather than on first selection. It is one small GET.
  const [terminalMounted, setTerminalMounted] = useState(false);
  // Marks the top of the loaded transcript: the history loader watches it and
  // fetches the next older page when the reader reaches it.
  const historySentinelRef = useRef<HTMLDivElement>(null);
  // The transcript element itself. Its top edge is the start of the loaded rows,
  // which is what the loader reveals when a page arrives while the reader is
  // scrolled above every row (see `lib/historyLoader.ts`).
  const transcriptRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    void terminalState.loadTerminals();
  }, []);

  const selectView = (next: WorkspaceView): void => {
    setView(next);
    workbench.closeContext();
    if (next === 'changes') setChangesMounted(true);
    if (next === 'terminal') setTerminalMounted(true);
  };

  // A file panel is kept mounted once it has been shown: its reads, its scroll
  // position and an unsaved draft all live inside it, and switching to the
  // conversation and back must not throw any of that away. A tab that was opened
  // and never selected has read nothing yet, so it mounts on first selection.
  const [mountedFiles, setMountedFiles] = useState<readonly string[]>([]);
  const selectedFile = workspaceTabs.fileViewPath(view);
  useEffect(() => {
    if (selectedFile === null) return;
    setMountedFiles((current) =>
      current.includes(selectedFile) ? current : [...current, selectedFile],
    );
  }, [selectedFile]);
  // Closing a tab unmounts its panel - the one moment that state is meant to go.
  useEffect(() => {
    setMountedFiles((current) => {
      const next = current.filter((path) => openFiles.includes(path));
      return next.length === current.length ? current : next;
    });
  }, [openFiles]);

  // A tab that disappears (the server disabled the terminal while this page was
  // open) must not leave the pane pointing at a view that no longer exists.
  useEffect(() => {
    if (view === 'terminal' && !terminalState.terminalsEnabled.value) setView('chat');
  }, [view, terminalState.terminalsEnabled.value]);

  const lastItem = chat.items[chat.items.length - 1];
  let tailSize = 0;
  if (lastItem?.kind === 'message') tailSize = lastItem.content.length + lastItem.reasoning.length;
  else if (lastItem?.kind === 'command') tailSize = lastItem.stdout.length + lastItem.stderr.length;
  else if (lastItem?.kind === 'tool') tailSize = lastItem.output.length;

  const scroller = useStickyBottom({
    sessionKey: chat.sessionId,
    contentKey: `${chat.items.length}:${tailSize}`,
    sentMessageTick: store.sentMessageTick.value,
  });

  // Older pages arrive as the reader reaches the top, instead of behind a
  // "Load older history" button. `version` is the row count so a page that
  // actually loaded can be followed by another; an error leaves it unchanged
  // and the loop stops.
  const history = store.sessionHistory.value?.id === currentId ? store.sessionHistory.value : null;
  useHistoryLoader({
    scroller: scroller.scrollRef,
    sentinel: historySentinelRef,
    content: transcriptRef,
    canLoad: history !== null && history.before !== null,
    loading: history?.loading ?? false,
    load: actions.loadOlderSession,
    version: history?.messages.length ?? 0,
  });

  // The dock floats over the transcript, so the scroller reserves its real
  // height: measured, because the box grows with the draft and the chip row.
  const panelRef = useRef<HTMLDivElement>(null);
  const centerBodyRef = useRef<HTMLDivElement>(null);
  const dockRef = useRef<HTMLDivElement>(null);
  useHeightVariable({ ref: dockRef, target: panelRef, name: 'composer-h' });

  const showLoading = store.chatLoading.value && currentId !== null;
  const hasItems = chat.items.length > 0;
  return (
    <main class={`chat-main${isWorkbench ? ' is-workbench' : ''}`} id="chat-main" aria-label="Workspace" tabIndex={-1} data-workspace-mode={mode}>
      <SessionHeader view={view} onShowChanges={showChanges} onSelectView={selectView} controlPlane={!isWorkbench} />

      <div
        ref={centerBodyRef}
        class={`center-body${isWorkbench && view === 'chat' && !store.inspectorCollapsed.value ? ' has-result' : ''}`}
        data-view={view}
        data-workspace-mode={mode}
      >
        <div
          class="center-panel chat-panel"
          id="workspace-panel-chat"
          role="tabpanel"
          aria-labelledby="workspace-tab-chat"
          hidden={view !== 'chat'}
          ref={panelRef}
          /* History is a reading state, not a reason to put text behind controls. */
          data-reading-history={scroller.scrolledAway ? 'true' : 'false'}
        >
          <div class="chat-body">
            {scroller.showJump ? (
              <button type="button" class="jump-latest" onClick={scroller.jumpToLatest}>
                <ChevronDownIcon size={14} />
                Jump to latest
              </button>
            ) : null}
            <div class="chat-scroll" ref={scroller.scrollRef} tabIndex={0} role="region" aria-label="Transcript">
              <div class="chat-inner">
                {/* Always mounted so it can speak the transitions, not just the state. */}
                <LiveAnnouncer chat={chat} />
                <ConnectionBanner />
                <ProviderBanner />

                {showLoading ? <ChatSkeleton /> : null}

                {!showLoading && store.chatError.value ? (
                  <div class="run-error-banner" role="alert">
                    <div class="run-error-head">
                      <span class="run-error-kicker">
                        <AlertIcon size={14} /> Could not load this session
                      </span>
                    </div>
                    <p class="run-error-message">{store.chatError.value}</p>
                    <button
                      type="button"
                      class="button button-small"
                      onClick={() => currentId && void actions.openSession(currentId, { silent: false, force: true })}
                    >
                      <RefreshIcon size={13} /> Retry
                    </button>
                  </div>
                ) : null}

                {!showLoading && history !== null && history.messages.length > 0 ? (
                  <div class="history-pagination" ref={historySentinelRef}>
                    {history.before !== null ? (
                      <p class="history-loading" role="status" aria-live="polite">
                        {history.loading ? 'Loading earlier messages…' : 'Scroll up to load earlier messages'}
                      </p>
                    ) : (
                      <p class="history-start">Beginning of the conversation</p>
                    )}
                    {history.error ? <p role="alert">{history.error}</p> : null}
                  </div>
                ) : null}
                {!showLoading && !store.chatError.value ? (
                  currentId === null ? (
                    drafting ? (
                      // An unsent draft: same empty state, but a prompt fills the
                      // box directly - there is already a conversation to type in.
                      emptyContent ?? <EmptyState variant="empty" />
                    ) : (
                      // Nothing open: the same mock empty state, but a prompt
                      // opens a draft first before filling the composer.
                      emptyContent ?? <EmptyState variant="no-session" />
                    )
                  ) : hasItems ? (
                    // A malformed item that slipped past the reducer must not take
                    // the composer and the rails down with it.
                    <ErrorBoundary scope="This conversation">
                      <ChatTranscript
                        items={chat.items}
                        resolvingApprovals={store.resolvingApprovals.value}
                        onApprovalDecision={decideApproval}
                        onRewind={actions.openRewind}
                        viewportRef={scroller.scrollRef}
                        contentRef={transcriptRef}
                        sessionKey={chat.sessionId}
                      />
                    </ErrorBoundary>
                  ) : chat.running ? (
                    // A live run with nothing persisted yet is not an empty session:
                    // the starter prompts would invite a second message mid-turn.
                    <ChatSkeleton />
                  ) : (
                    emptyContent ?? <EmptyState variant="empty" />
                  )
                ) : null}

                <RunErrorBanner />


              </div>
            </div>
          </div>

          <div class="composer-wrap" ref={dockRef}>
            <div class="composer-inner">
              <Composer
                disabledReason={store.composerDisabledReason.value}
                running={chat.running}
                stopping={store.stopping.value}
                status={store.activity.value}
                onSend={actions.sendChatMessage}
                onQueue={actions.queueChatMessage}
                queued={chat.queued}
                onRecall={actions.recallQueuedMessage}
                recalling={store.recallingQueued.value}
                onStop={() => void actions.stopRun()}
                onOpenSettings={actions.openSettings}
                focusTick={store.composerFocusTick.value}
                usageLabel={totalUsage(chat)}
                loadMentionTree={mentionProjects ? projectMentionLoader(mentionProjects) : undefined}
                onOpenUsage={isWorkbench ? () => workbench.openContext('usage') : undefined}
                mentionLabel={mentionProjects ? 'projects' : undefined}
              />
            </div>
          </div>
        </div>

        {changesMounted ? (
          <div
            class="center-panel workspace-view git-workspace"
            id="workspace-panel-changes"
            role="tabpanel"
            aria-labelledby="workspace-tab-changes"
            hidden={view !== 'changes'}
            tabIndex={0}
          >
            {/* The source-control manager owns the whole panel: its two regions
                scroll independently, so the tabpanel itself is not a scroll host
                (`.git-workspace` turns that off). */}
            <GitManager variant="page" active={view === 'changes'} />
          </div>
        ) : null}

        {terminalMounted ? (
          <div
            class="center-panel terminal-panel"
            id="workspace-panel-terminal"
            role="tabpanel"
            aria-labelledby="workspace-tab-terminal"
            hidden={view !== 'terminal'}
          >
            <TerminalTab />
          </div>
        ) : null}

        {openFiles.map((path) => {
          const panelView = workspaceTabs.fileView(path);
          return (
            <div
              key={path}
              class="center-panel file-workspace"
              id={`workspace-panel-${panelView}`}
              role="tabpanel"
              aria-labelledby={`workspace-tab-${panelView}`}
              hidden={view !== panelView}
            >
              {mountedFiles.includes(path) ? (
                // One bad document must not take the tabs beside it down: the
                // viewer fetches and renders a file someone else wrote.
                <ErrorBoundary scope={`The ${fileName(path)} tab`}>
                  <FileTabView path={path} />
                </ErrorBoundary>
              ) : null}
            </div>
          );
        })}

        {isWorkbench ? (
          <>
            {view === 'chat' && !store.inspectorCollapsed.value ? (
              <ResultResizer
                containerRef={centerBodyRef}
                projectId={store.currentProject.value?.id ?? null}
                preferredWidth={store.currentProject.value?.result_panel_width}
              />
            ) : null}
            <ErrorBoundary scope="The inspector">
              <Inspector />
            </ErrorBoundary>
            <ErrorBoundary scope="Workspace context">
              <ContextDrawer onRevealResult={showResult} onShowConversation={showConversation} />
            </ErrorBoundary>
          </>
        ) : null}
      </div>
    </main>
  );
}