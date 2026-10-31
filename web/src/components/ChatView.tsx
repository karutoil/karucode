import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import type { ChatItem } from '../state/chat';
import { copyText } from '../lib/clipboard';
import { useHeightVariable } from '../lib/dockHeight';
import { modKey } from '../lib/platform';
import { useStickyBottom } from '../lib/stickyBottom';
import { getGitStatus } from '../api/endpoints';
import * as actions from '../state/actions';
import * as terminalState from '../state/terminals';
import { totalUsage } from '../state/selectors';
import * as store from '../state/store';
import { ChangesTab } from './ChangesTab';
import { ChatTranscript } from './ChatTranscript';
import { Composer } from './Composer';
import { EmptyState } from './EmptyState';
import { ErrorBoundary } from './ErrorBoundary';
import { LiveAnnouncer } from './LiveAnnouncer';
import { Tabs, type TabItem } from './Tabs';
import { TerminalTab } from './TerminalTab';
import {
  AlertIcon,
  ChatIcon,
  ChevronDownIcon,
  CircleCheckIcon,
  CopyIcon,
  GitIcon,
  MenuIcon,
  PaletteIcon,
  PanelIcon,
  RefreshIcon,
  SearchIcon,
  TerminalIcon,
} from './icons';
import { activeThemeName } from '../theme/engine';

/**
 * The centre-pane views of the workspace (the mock's workspace tabs).
 *
 * `terminal` is the odd one out: it is a real shell, not a view over the
 * session, so it works with no session open and its state belongs to the server
 * rather than to the conversation.
 */
export type WorkspaceView = 'chat' | 'changes' | 'terminal';

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

function SessionHeader({ view }: { view: WorkspaceView }): JSX.Element {
  const session = store.activeSession.value;
  const activity = store.activity.value;
  const narrow = store.narrowViewport.value;
  const project = store.currentProject.value;
  const projectName = project?.name ?? 'karucode';
  const railExpanded = narrow ? store.mobileSheet.value === 'sessions' : !store.railCollapsed.value;
  const inspectorExpanded = narrow
    ? store.mobileSheet.value === 'inspector'
    : !store.inspectorCollapsed.value;

  const title =
    view === 'changes'
      ? 'Review changes'
      : view === 'terminal'
        ? 'Terminal'
        : session
          ? session.title || 'Untitled session'
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

      <div class="head-title-wrap">
        <div class="breadcrumb">
          <strong title={project?.path ?? projectName}>{projectName}</strong>
          <span class="crumb-sep" aria-hidden="true">
            /
          </span>
          <span>sessions</span>
          <span class="crumb-sep" aria-hidden="true">
            /
          </span>
          <span class={`head-activity status-${activity.kind}`}>{activityWord}</span>
        </div>
        <h1 class="main-title" title={title}>
          {title}
        </h1>
      </div>

      <div class="header-buttons">
        <button
          type="button"
          class="head-btn theme-header-btn"
          onClick={actions.openThemeStudio}
          aria-haspopup="dialog"
          aria-keyshortcuts="Control+Shift+T Meta+Shift+T"
          title={`Theme studio — ${modKey()}+Shift+T`}
        >
          <PaletteIcon size={15} />
          <span class="theme-header-name">{activeThemeName.value}</span>
        </button>
        <button
          type="button"
          class="head-btn command-head-btn"
          onClick={actions.togglePalette}
          aria-haspopup="dialog"
          aria-keyshortcuts="Control+K Meta+K"
          title={`Command palette — ${modKey()}+K`}
        >
          <SearchIcon size={15} />
          <span>Commands</span>
          <span class="keycap">{modKey()} K</span>
        </button>
        <button
          type="button"
          class="head-btn accent-head"
          onClick={() => void copyConversation()}
          disabled={store.chat.value.items.length === 0 || view !== 'chat'}
          title="Copy this conversation as Markdown"
        >
          <CopyIcon size={15} />
          <span class="head-btn-label">Copy</span>
        </button>
        <div class="header-divider" aria-hidden="true" />
        <button
          type="button"
          class="head-btn head-icon"
          onClick={actions.toggleInspector}
          aria-label="Toggle workspace inspector"
          aria-expanded={inspectorExpanded}
          title={`Inspector — ${modKey()}+J`}
        >
          <PanelIcon size={15} />
        </button>
      </div>
    </header>
  );
}

function WorkspaceTabs({ view, onChange }: { view: WorkspaceView; onChange: (next: WorkspaceView) => void }): JSX.Element {
  const activity = store.activity.value;
  const changeCount = useChangeCount();
  const items: TabItem<WorkspaceView>[] = [
    { id: 'chat', label: 'Conversation', icon: <ChatIcon size={15} /> },
    { id: 'changes', label: 'Changes', icon: <GitIcon size={15} />, count: changeCount ?? undefined },
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
  const note = activity.kind === 'idle' ? 'Ready to work' : activity.label;

  return (
    <div class="workspace-tabbar">
      <Tabs
        items={items}
        active={view}
        onChange={onChange}
        label="Workspace views"
        idPrefix="workspace"
        variant="workspace"
      />
      <div class="tab-fill" />
      <p class="tab-note">
        {activity.kind === 'idle' ? (
          <CircleCheckIcon size={13} />
        ) : (
          <span class={`status-dot status-${activity.kind}`} aria-hidden="true" />
        )}
        {note}
      </p>
    </div>
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

export function ChatView(): JSX.Element {
  const session = store.activeSession.value;
  const currentId = store.currentSessionId.value;
  const chat = store.chat.value;
  const [view, setView] = useState<WorkspaceView>('chat');
  // Lazy panel: Changes mounts its git probe only after the user asks for it.
  const [changesMounted, setChangesMounted] = useState(false);
  // The terminal list is needed to decide whether the tab exists at all, so it
  // is fetched on mount rather than on first selection. It is one small GET.
  const [terminalMounted, setTerminalMounted] = useState(false);

  useEffect(() => {
    void terminalState.loadTerminals();
  }, []);

  const selectView = (next: WorkspaceView): void => {
    setView(next);
    if (next === 'changes') setChangesMounted(true);
    if (next === 'terminal') setTerminalMounted(true);
  };

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

  // The dock floats over the transcript, so the scroller reserves its real
  // height: measured, because the box grows with the draft and the chip row.
  const panelRef = useRef<HTMLDivElement>(null);
  const dockRef = useRef<HTMLDivElement>(null);
  useHeightVariable({ ref: dockRef, target: panelRef, name: 'composer-h' });

  const showLoading = store.chatLoading.value && currentId !== null;
  const hasItems = chat.items.length > 0;
  const workspace =
    session?.workspace ??
    store.currentProject.value?.path ??
    store.health.value?.workspace ??
    store.config.value?.workspace ??
    '';

  return (
    <main class="chat-main" id="chat-main" aria-label="Workspace" tabIndex={-1}>
      <SessionHeader view={view} />
      <WorkspaceTabs view={view} onChange={selectView} />

      <div class="center-body">
        <div
          class="center-panel chat-panel"
          id="workspace-panel-chat"
          role="tabpanel"
          aria-labelledby="workspace-tab-chat"
          hidden={view !== 'chat'}
          ref={panelRef}
          /* Scrolled into history: the dock thins out so the transcript shows
           * through it. Typing (focus inside the box) restores it in CSS. */
          data-translucent={scroller.scrolledAway ? 'true' : 'false'}
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

                {!showLoading && !store.chatError.value ? (
                  currentId === null ? (
                    // Nothing open: the same mock empty state, but a prompt opens
                    // the session first before filling the composer.
                    <EmptyState variant="no-session" />
                  ) : hasItems ? (
                    // A malformed item that slipped past the reducer must not take
                    // the composer and the rails down with it.
                    <ErrorBoundary scope="This conversation">
                      <ChatTranscript
                        items={chat.items}
                        resolvingApprovals={store.resolvingApprovals.value}
                        onApprovalDecision={(id, decision) => void actions.resolveApproval(id, decision)}
                        onRewind={actions.openRewind}
                      />
                    </ErrorBoundary>
                  ) : chat.running ? (
                    // A live run with nothing persisted yet is not an empty session:
                    // the starter prompts would invite a second message mid-turn.
                    <ChatSkeleton />
                  ) : (
                    <EmptyState variant="empty" />
                  )
                ) : null}

                <RunErrorBanner />

                {session && hasItems && workspace ? (
                  <p class="transcript-foot">
                    Working in <span title={workspace}>{workspace}</span>
                  </p>
                ) : null}
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
              />
            </div>
          </div>
        </div>

        {changesMounted ? (
          <div
            class="center-panel workspace-view"
            id="workspace-panel-changes"
            role="tabpanel"
            aria-labelledby="workspace-tab-changes"
            hidden={view !== 'changes'}
            tabIndex={0}
          >
            {/* The page variant owns the mock's `.secondary-panel` chrome
                (kicker, title, description, actions, change cards). */}
            <ChangesTab variant="page" />
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

      </div>
    </main>
  );
}