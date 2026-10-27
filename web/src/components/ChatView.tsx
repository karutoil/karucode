import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { formatCount } from '../lib/format';
import * as actions from '../state/actions';
import { totalUsage } from '../state/selectors';
import * as store from '../state/store';
import { ChatTranscript } from './ChatTranscript';
import { Composer } from './Composer';
import { AlertIcon, PanelIcon, RefreshIcon, SidebarIcon } from './icons';

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
  const reconnecting = connection.stream === 'reconnecting';
  if (!offline && !reconnecting) return null;
  return (
    <div class={`banner ${offline ? 'banner-danger' : 'banner-warn'}`} role="status">
      <AlertIcon size={15} />
      <div class="banner-body">
        {offline ? (
          <>
            <strong>Disconnected from karucode.</strong> Showing the last known state; retrying in the
            background.
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

function NoSessionState(): JSX.Element {
  const workspace = store.health.value?.workspace ?? store.config.value?.workspace ?? '';
  return (
    <div class="empty-state">
      <p class="eyebrow">No session open</p>
      <h2>Start a session</h2>
      <p>
        A session keeps one conversation, its runs and approvals. karucode works inside{' '}
        {workspace ? <code class="inline-path">{workspace}</code> : 'the configured workspace'} and asks before it
        changes files or runs commands.
      </p>
      <div class="empty-actions">
        <button type="button" class="button button-primary" onClick={() => void actions.createSession()}>
          New session
        </button>
        <button type="button" class="button" onClick={actions.toggleRail}>
          {store.narrowViewport.value ? 'Show sessions' : 'Show session rail'}
        </button>
      </div>
      <p class="empty-hint">
        <kbd>Cmd</kbd>/<kbd>Ctrl</kbd> + <kbd>N</kbd> creates a session · <kbd>Cmd</kbd>/<kbd>Ctrl</kbd> +{' '}
        <kbd>K</kbd> opens the command palette.
      </p>
    </div>
  );
}

function EmptySessionState(): JSX.Element {
  const workspace =
    store.activeSession.value?.workspace ??
    store.health.value?.workspace ??
    store.config.value?.workspace ??
    '';
  return (
    <div class="empty-state">
      <p class="eyebrow">Session ready</p>
      <h2>Nothing sent yet</h2>
      <p>
        Ask a question or describe a change. karucode can read files in{' '}
        {workspace ? <code class="inline-path">{workspace}</code> : 'the workspace'}, propose edits and run
        commands.
      </p>
      <ul class="empty-list">
        <li>Commands and mutating tools stop for approval unless the policy allows them.</li>
        <li>Output, reasoning and command streams appear here as they happen.</li>
        <li>
          Open the inspector with <kbd>Cmd</kbd>/<kbd>Ctrl</kbd> + <kbd>J</kbd> to browse files, changes and
          subagents.
        </li>
      </ul>
      <p class="empty-hint">Shell commands are not sandboxed — read approvals before allowing them.</p>
    </div>
  );
}

function SessionHeader(): JSX.Element {
  const session = store.activeSession.value;
  const activity = store.activity.value;
  const narrow = store.narrowViewport.value;
  const railExpanded = narrow ? store.mobileSheet.value === 'sessions' : !store.railCollapsed.value;
  const inspectorExpanded = narrow
    ? store.mobileSheet.value === 'inspector'
    : !store.inspectorCollapsed.value;
  const workspace =
    session?.workspace ?? store.health.value?.workspace ?? store.config.value?.workspace ?? '';

  return (
    <header class="topbar">
      <button
        type="button"
        class="icon-button"
        onClick={actions.toggleRail}
        aria-label="Toggle sessions panel"
        aria-expanded={railExpanded}
        title="Toggle sessions — Cmd/Ctrl+B"
      >
        <SidebarIcon />
      </button>

      <div class="topbar-titles">
        <h1 class="topbar-title">{session?.title || 'karucode'}</h1>
        <p class="topbar-subtitle">
          {session ? (
            <>
              <span class="topbar-workspace" title={workspace}>
                {workspace || 'workspace'}
              </span>
              <span class="meta-sep" aria-hidden="true">
                ·
              </span>
              <span>{formatCount(session.message_count)}</span>
              <span class="meta-sep" aria-hidden="true">
                ·
              </span>
              <span class={`topbar-status status-${activity.kind}`}>
                <span class={`status-dot status-${activity.kind}`} aria-hidden="true" />
                {activity.label}
              </span>
            </>
          ) : (
            'Local coding agent'
          )}
        </p>
      </div>

      <div class="topbar-actions">
        <button
          type="button"
          class="icon-button"
          onClick={actions.toggleInspector}
          aria-label="Toggle workspace inspector"
          aria-expanded={inspectorExpanded}
          title="Toggle inspector — Cmd/Ctrl+J"
        >
          <PanelIcon />
        </button>
      </div>
    </header>
  );
}

export function ChatView(): JSX.Element {
  const session = store.activeSession.value;
  const currentId = store.currentSessionId.value;
  const chat = store.chat.value;
  const scroller = useRef<HTMLDivElement>(null);
  const previousSession = useRef<string | null>(null);

  const lastItem = chat.items[chat.items.length - 1];
  let tailSize = 0;
  if (lastItem?.kind === 'message') tailSize = lastItem.content.length + lastItem.reasoning.length;
  else if (lastItem?.kind === 'command') tailSize = lastItem.stdout.length + lastItem.stderr.length;
  else if (lastItem?.kind === 'tool') tailSize = lastItem.output.length;

  useEffect(() => {
    const element = scroller.current;
    if (!element) return;
    const sessionChanged = previousSession.current !== chat.sessionId;
    previousSession.current = chat.sessionId;
    const nearBottom = element.scrollHeight - element.scrollTop - element.clientHeight < 180;
    if (sessionChanged || nearBottom) element.scrollTop = element.scrollHeight;
  }, [chat.sessionId, chat.items.length, tailSize]);

  const showLoading = store.chatLoading.value && currentId !== null;
  const hasItems = chat.items.length > 0;
  const workspace =
    session?.workspace ?? store.health.value?.workspace ?? store.config.value?.workspace ?? '';

  return (
    <main class="chat-main" id="chat-main" aria-label="Conversation" tabIndex={-1}>
      <SessionHeader />
      <div class="chat-scroll" ref={scroller}>
        <div class="chat-inner">
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
              <NoSessionState />
            ) : hasItems ? (
              <ChatTranscript
                items={chat.items}
                resolvingApprovals={store.resolvingApprovals.value}
                onApprovalDecision={(id, decision) => void actions.resolveApproval(id, decision)}
              />
            ) : (
              <EmptySessionState />
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

      <div class="composer-wrap">
        <div class="composer-inner">
          <Composer
            disabledReason={store.composerDisabledReason.value}
            running={chat.running}
            stopping={store.stopping.value}
            status={store.activity.value}
            onSend={actions.sendChatMessage}
            onStop={() => void actions.stopRun()}
            onOpenSettings={actions.openSettings}
            focusTick={store.composerFocusTick.value}
            usageLabel={totalUsage(chat)}
          />
        </div>
      </div>
    </main>
  );
}