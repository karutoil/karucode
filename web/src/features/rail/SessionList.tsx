import { useMemo, useState } from 'preact/hooks';
import type { JSX } from 'preact';
import { modKey } from '../../lib/platform';
import type { SessionAnnotation, SessionSummary } from '../../api/types';
import { pluginText, UI_REPLACE_NEW_SESSION_LABEL } from '../../components/uiContributions';
import * as store from '../../state/store';
import { sessionFacts } from '../../state/sessionFacts';
import { Dialog } from '../../components/Dialog';
import { PlusIcon } from '../../components/icons';
import { passesFilter, sessionGroup, type SessionFilter } from './helpers';
import { SessionGroup } from './SessionGroup';

export interface SessionListProps {
  sessions: SessionSummary[];
  currentId: string | null;
  now: number;
  loading: boolean;
  workspace?: string;
  /** Case-insensitive title filter typed into the rail's search field. */
  query?: string;
  /** The chip filter in force; `all` shows everything. */
  filter?: SessionFilter;
  /**
   * Plugin annotations for **these** sessions (§R4). Optional: a caller with no
   * answer draws no chips, which is what the expanded rail wants while the plugin
   * is down. Resolved by the caller, never from the store, because the fly-out
   * draws a project that is not the open one.
   */
  annotations?: SessionAnnotation[];
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
  query = '',
  filter = 'all',
  annotations,
  onCreate,
  onSelect,
  onRename,
  onDelete,
}: SessionListProps): JSX.Element {
  const [pendingDelete, setPendingDelete] = useState<{
    session: SessionSummary;
    trigger: HTMLElement | null;
  } | null>(null);
  // The per-session fingerprints under each title. Read lazily and bounded (see
  // `state/sessionFacts.ts`): the rail still renders from the list endpoint, so
  // a slow or refused read only costs the second line.
  const facts = sessionFacts.value;
  // Chips by session id, built once per annotations change rather than searched
  // per row: a long rail would otherwise scan the array for every row.
  const chips = useMemo(() => {
    const byId = new Map<string, { text: string; tone: string }[]>();
    for (const annotation of annotations ?? []) {
      if (annotation.badges?.length) byId.set(annotation.id, annotation.badges);
    }
    return byId;
  }, [annotations]);
  const badgesFor = (id: string) => chips.get(id) ?? [];

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

  const needle = query.trim().toLowerCase();
  const unreadCounts = store.unread.value;
  const visible = sessions.filter((session) => {
    if (!passesFilter(session, filter, unreadCounts[session.id] ?? 0)) return false;
    return needle === '' || (session.title || '').toLowerCase().includes(needle);
  });

  // Preserve the server order (newest first) inside each date bucket.
  const groups: { label: string; items: SessionSummary[] }[] = [];
  for (const session of visible) {
    const label = sessionGroup(session.updated_at, now);
    const last = groups[groups.length - 1];
    if (last && last.label === label) last.items.push(session);
    else groups.push({ label, items: [session] });
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
            <PlusIcon size={14} />{' '}
            {pluginText(store.pluginUi.value, UI_REPLACE_NEW_SESSION_LABEL, 'New session')}
          </button>
          <p class="rail-hint">
            Create with <kbd>{modKey()}</kbd> + <kbd>N</kbd>, or pick one from the command palette{' '}
            <kbd>{modKey()}</kbd> + <kbd>K</kbd>.
          </p>
        </div>
      ) : visible.length === 0 ? (
        <div class="rail-empty" role="status">
          <p class="rail-empty-title">No matching sessions</p>
          <p class="rail-empty-body">
            {query.trim() === '' ? (
              <>No session in this project matches the {filter} filter right now.</>
            ) : (
              <>
                Nothing in this project matches <strong>{query.trim()}</strong>. Searches only look at
                session titles.
              </>
            )}
          </p>
          <button type="button" class="button" onClick={onCreate}>
            <PlusIcon size={14} />{' '}
            {pluginText(store.pluginUi.value, UI_REPLACE_NEW_SESSION_LABEL, 'New session')}
          </button>
        </div>
      ) : (
        groups.map((group) => (
          <SessionGroup
            key={group.label}
            label={group.label}
            items={group.items}
            currentId={currentId}
            now={now}
            facts={facts}
            unreadCounts={unreadCounts}
            badgesFor={badgesFor}
            onSelect={onSelect}
            onRename={onRename}
            onRequestDelete={(session, trigger) => setPendingDelete({ session, trigger })}
          />
        ))
      )}

      <Dialog open={pendingDelete !== null} title="Delete session?" onClose={closeDelete} class="dialog-confirm">
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
