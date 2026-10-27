import { useEffect, useRef, useState } from 'preact/hooks';
import type { JSX } from 'preact';
import { useFocusTrap } from '../lib/dom';
import type { SessionSummary } from '../api/types';
import { formatCount, relativeTime } from '../lib/format';
import * as store from '../state/store';
import * as actions from '../state/actions';
import { Dialog } from './Dialog';
import { CheckIcon, CloseIcon, KebabIcon, PlusIcon, SettingsIcon } from './icons';

interface SessionRowProps {
  session: SessionSummary;
  active: boolean;
  now: number;
  onSelect: (id: string) => void;
  onRename: (id: string, title: string) => void | Promise<boolean>;
  onRequestDelete: (session: SessionSummary, trigger: HTMLElement | null) => void;
}

function SessionRow({ session, active, now, onSelect, onRename, onRequestDelete }: SessionRowProps): JSX.Element {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(session.title);
  const [menuOpen, setMenuOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const skipBlur = useRef(false);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  useEffect(() => {
    if (!menuOpen) return;
    const first = menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]');
    first?.focus();
    const onDocumentClick = (event: MouseEvent): void => {
      if (!menuRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        setMenuOpen(false);
        triggerRef.current?.focus();
      }
    };
    document.addEventListener('mousedown', onDocumentClick);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', onDocumentClick);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [menuOpen]);

  useEffect(() => {
    if (!editing) setDraft(session.title);
  }, [session.title, editing]);

  const startRename = (): void => {
    skipBlur.current = false;
    setMenuOpen(false);
    setDraft(session.title);
    setEditing(true);
  };

  const cancelRename = (): void => {
    skipBlur.current = true;
    setEditing(false);
    setDraft(session.title);
  };

  const commitRename = (): void => {
    if (skipBlur.current) {
      skipBlur.current = false;
      return;
    }
    const next = draft.trim();
    setEditing(false);
    if (next !== '' && next !== session.title) void onRename(session.id, next);
    else setDraft(session.title);
  };

  const statusTone = session.running
    ? 'running'
    : session.last_run_status === 'failed'
      ? 'danger'
      : session.last_run_status === 'interrupted'
        ? 'warn'
        : null;

  return (
    <li class={`session-row${active ? ' is-active' : ''}`} data-session-id={session.id}>
      {editing ? (
        <form
          class="session-rename"
          onSubmit={(event) => {
            event.preventDefault();
            commitRename();
          }}
        >
          <input
            ref={inputRef}
            class="session-rename-input"
            value={draft}
            aria-label={`Rename session ${session.title}`}
            maxLength={200}
            onInput={(event) => setDraft(event.currentTarget.value)}
            onBlur={commitRename}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                event.preventDefault();
                cancelRename();
              }
            }}
          />
        </form>
      ) : (
        <button
          type="button"
          class="session-open"
          onClick={() => onSelect(session.id)}
          onDblClick={startRename}
          aria-current={active ? 'true' : undefined}
        >
          <span class="session-title">{session.title || 'Untitled session'}</span>
          <span class="session-meta">
            {statusTone ? (
              <span class={`status-dot status-${statusTone}`} aria-hidden="true" />
            ) : null}
            {session.running ? <span class="sr-only">Running. </span> : null}
            <span>{relativeTime(session.updated_at, now)}</span>
            <span class="meta-sep" aria-hidden="true">
              ·
            </span>
            <span title={formatCount(session.message_count)}>
              {session.message_count}
              <span class="sr-only"> messages</span>
            </span>
            {session.last_run_status === 'failed' ? <span class="sr-only">Last run failed.</span> : null}
            {session.last_run_status === 'interrupted' ? (
              <span class="sr-only">Last run interrupted.</span>
            ) : null}
          </span>
        </button>
      )}

      {!editing ? (
        <div class="session-menu-wrap" ref={menuRef}>
          <button
            type="button"
            ref={triggerRef}
            class="icon-button session-menu-trigger"
            aria-label={`Actions for ${session.title || 'untitled session'}`}
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            onClick={() => setMenuOpen((open) => !open)}
          >
            <KebabIcon size={15} />
          </button>
          {menuOpen ? (
            <div
              class="session-menu"
              role="menu"
              aria-label={`Session actions for ${session.title}`}
              onKeyDown={(event) => {
                const items = Array.from(
                  menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [],
                );
                if (items.length === 0) return;
                const index = items.indexOf(document.activeElement as HTMLButtonElement);
                let next = index;
                if (event.key === 'ArrowDown') next = (index + 1) % items.length;
                else if (event.key === 'ArrowUp') next = (index - 1 + items.length) % items.length;
                else if (event.key === 'Home') next = 0;
                else if (event.key === 'End') next = items.length - 1;
                else if (event.key === 'Tab') {
                  setMenuOpen(false);
                  return;
                } else return;
                event.preventDefault();
                items[next]?.focus();
              }}
            >
              <button type="button" role="menuitem" class="menu-item" onClick={startRename}>
                Rename
              </button>
              <button
                type="button"
                role="menuitem"
                class="menu-item menu-item-danger"
                onClick={() => {
                  triggerRef.current?.focus();
                  setMenuOpen(false);
                  onRequestDelete(session, triggerRef.current);
                }}
              >
                Delete…
              </button>
            </div>
          ) : null}
        </div>
      ) : null}
    </li>
  );
}

export interface SessionListProps {
  sessions: SessionSummary[];
  currentId: string | null;
  now: number;
  loading: boolean;
  workspace?: string;
  onCreate: () => void;
  onSelect: (id: string) => void;
  onRename: (id: string, title: string) => void | Promise<boolean>;
  onDelete: (id: string) => void | Promise<unknown>;
}

/** Presentational session list; exported for tests. */
export function SessionList({
  sessions,
  currentId,
  now,
  loading,
  workspace,
  onCreate,
  onSelect,
  onRename,
  onDelete,
}: SessionListProps): JSX.Element {
  const [pendingDelete, setPendingDelete] = useState<{
    session: SessionSummary;
    trigger: HTMLElement | null;
  } | null>(null);

  const closeDelete = (): void => {
    const trigger = pendingDelete?.trigger;
    setPendingDelete(null);
    if (trigger) {
      window.setTimeout(() => {
        if (trigger.isConnected) trigger.focus();
      }, 0);
    }
  };

  if (loading && sessions.length === 0) {
    return (
      <div class="session-list-loading" aria-busy="true" aria-label="Loading sessions">
        {[0, 1, 2, 3].map((index) => (
          <div class="skeleton skeleton-session" key={index} />
        ))}
      </div>
    );
  }

  return (
    <div class="session-list-wrap">
      {sessions.length === 0 ? (
        <div class="rail-empty">
          <p class="rail-empty-title">No sessions yet</p>
          <p class="rail-empty-body">
            A session keeps one conversation, its runs and the files it touches
            {workspace ? (
              <>
                {' '}
                in <code class="inline-path">{workspace}</code>
              </>
            ) : null}
            .
          </p>
          <button type="button" class="button button-primary" onClick={onCreate}>
            <PlusIcon size={14} /> New session
          </button>
          <p class="rail-hint">
            Create with <kbd>Cmd</kbd>/<kbd>Ctrl</kbd> + <kbd>N</kbd>, or pick one from the command palette{' '}
            <kbd>Cmd</kbd>/<kbd>Ctrl</kbd> + <kbd>K</kbd>.
          </p>
        </div>
      ) : (
        <ul class="session-list" aria-label="Sessions">
          {sessions.map((session) => (
            <SessionRow
              key={session.id}
              session={session}
              active={session.id === currentId}
              now={now}
              onSelect={onSelect}
              onRename={onRename}
              onRequestDelete={(session, trigger) => setPendingDelete({ session, trigger })}
            />
          ))}
        </ul>
      )}

      <Dialog
        open={pendingDelete !== null}
        title="Delete session?"
        onClose={closeDelete}
        class="dialog-confirm"
      >
        {pendingDelete ? (
          <div class="confirm-body">
            <p>
              Delete <strong>{pendingDelete.session.title || 'this session'}</strong>? Its messages, runs and
              approvals are removed. This cannot be undone.
            </p>
            <div class="dialog-actions">
              <button type="button" class="button" data-autofocus onClick={closeDelete}>
                Cancel
              </button>
              <button
                type="button"
                class="button button-danger"
                onClick={() => {
                  const target = pendingDelete.session;
                  closeDelete();
                  void onDelete(target.id);
                }}
              >
                Delete session
              </button>
            </div>
          </div>
        ) : null}
      </Dialog>
    </div>
  );
}

function connectionTone(): string {
  const value = store.connection.value;
  if (value.api === 'offline') return 'danger';
  if (value.stream === 'unauthorized') return 'danger';
  if (value.stream === 'reconnecting' || value.stream === 'connecting') return 'warn';
  if (value.stream === 'open') return 'ok';
  if (value.api === 'online') return 'ok';
  return 'muted';
}

export function SessionsRail(): JSX.Element {
  const sessions = store.sessions.value;
  const currentId = store.currentSessionId.value;
  const isSheetOpen = store.mobileSheet.value === 'sessions';
  const isNarrow = store.narrowViewport.value;
  const railRef = useRef<HTMLElement>(null);
  useFocusTrap(railRef, isNarrow && isSheetOpen, actions.closeSheets);

  return (
    <aside
      ref={railRef}
      class={`rail${isSheetOpen ? ' is-open' : ''}`}
      aria-label="Sessions"
      role={isSheetOpen ? 'dialog' : undefined}
      aria-modal={isSheetOpen ? 'true' : undefined}
    >
      <div class="rail-inner">
        <header class="rail-header">
          <span class="brand">
            <span class="brand-dot" aria-hidden="true" />
            <span class="brand-name">karucode</span>
          </span>
          <div class="rail-header-actions">
            <button
              type="button"
              class="icon-button"
              onClick={() => void actions.createSession()}
              aria-label="New session (Command or Control + N)"
              title="New session — Cmd/Ctrl+N"
            >
              <PlusIcon />
            </button>
            {isNarrow ? (
              <button
                type="button"
                class="icon-button rail-close"
                aria-label="Close sessions panel"
                data-autofocus
                onClick={actions.closeSheets}
              >
                <CloseIcon />
              </button>
            ) : null}
          </div>
        </header>

        <nav class="session-nav" aria-label="Session list">
          <SessionList
            sessions={sessions}
            currentId={currentId}
            now={store.clock.value}
            loading={store.sessionsLoading.value}
            workspace={store.health.value?.workspace ?? store.config.value?.workspace}
            onCreate={() => void actions.createSession()}
            onSelect={actions.switchSession}
            onRename={actions.renameSessionById}
            onDelete={actions.deleteSessionById}
          />
        </nav>

        <footer class="rail-footer">
          <div class="rail-status-row">
            <span class={`status-dot status-${connectionTone()}`} aria-hidden="true" />
            <span class="rail-connection">{store.connectionLabel.value}</span>
            <span class="rail-spacer" />
            {store.health.value ? <span class="rail-version">v{store.health.value.version}</span> : null}
          </div>
          <div class="rail-footer-row">
            <span class={store.providerConfigured.value ? 'rail-provider' : 'rail-provider is-warn'}>
              {store.providerConfigured.value ? (
                <>
                  <CheckIcon size={12} /> Provider configured
                </>
              ) : (
                'No provider configured'
              )}
            </span>
            <button type="button" class="link-button" onClick={actions.openSettings}>
              <SettingsIcon size={12} /> Settings
            </button>
          </div>
          {store.connection.value.lastError ? (
            <p class="rail-error" title={store.connection.value.lastError}>
              {store.connection.value.lastError}
            </p>
          ) : null}
        </footer>
      </div>
    </aside>
  );
}