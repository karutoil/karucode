import type { JSX } from 'preact';
import { relativeTime } from '../lib/format';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Dialog } from './Dialog';
import { BellIcon, CheckIcon } from './icons';
import '../styles/notifications.css';

/** The tone each kind paints: a finished run is news, a failure is a failure. */
const KIND_TONES: Record<store.NotificationEntry['kind'], string> = {
  run_finished: 'ok',
  needs_input: 'warn',
  error: 'danger',
  info: 'muted',
};

/** What each kind means, in a word, for the row's chip. */
const KIND_LABELS: Record<store.NotificationEntry['kind'], string> = {
  run_finished: 'Finished',
  needs_input: 'Needs you',
  error: 'Failed',
  info: 'Notice',
};

/**
 * The notification centre: everything the server has told this browser, newest
 * first, with the unread ones marked.
 *
 * The reason it exists is the same reason the notifications do: a run finishes in a
 * project you are not looking at. A toast is gone in nine seconds, a desktop
 * notification may be refused, and a dot on a session row says *that* something
 * happened without saying what. This is the list you come back to.
 *
 * It reads `store.notifications`, which is the one source the per-session dot, the
 * per-project counter on the rail and the tab title are all derived from — so
 * marking a row read here clears those everywhere, and opening a session clears that
 * session's rows without a second code path.
 *
 * A row's session may belong to another project. Opening it therefore goes through
 * `openNotification`, which switches project first: the rail must never keep listing
 * one project while the chat shows a session from another.
 */
export function NotificationCenter(): JSX.Element {
  const open = store.notificationCenterOpen.value;
  const entries = store.notifications.value;
  const unread = store.unreadTotal.value;
  const now = store.clock.value;
  const projects = store.projects.value;

  const projectName = (projectId: string | null): string | null => {
    if (projectId === null) return null;
    return projects.find((project) => project.id === projectId)?.name ?? null;
  };

  return (
    <Dialog
      open={open}
      title="Notifications"
      onClose={actions.closeNotificationCenter}
      class="notification-center"
      description={
        unread > 0
          ? `${unread} unread, newest first.`
          : 'Newest first. Everything here has been read.'
      }
    >
      <div class="notification-toolbar">
        <span class="notification-count" aria-live="polite">
          <BellIcon size={14} />
          {entries.length === 0
            ? 'Nothing yet'
            : `${entries.length} ${entries.length === 1 ? 'notification' : 'notifications'}`}
        </span>
        <span class="notification-toolbar-actions">
          <button
            type="button"
            class="button"
            disabled={unread === 0}
            onClick={store.markAllNotificationsRead}
          >
            <CheckIcon size={13} /> Mark all read
          </button>
          <button
            type="button"
            class="button"
            disabled={entries.length === 0}
            onClick={store.clearNotifications}
          >
            Clear
          </button>
        </span>
      </div>

      {entries.length === 0 ? (
        <div class="rail-empty notification-empty">
          <p class="rail-empty-title">No notifications yet</p>
          <p class="rail-empty-body">
            One arrives when a run finishes, when a session parks on an approval, or when a run
            fails — including runs in projects you are not looking at.
          </p>
        </div>
      ) : (
        <ul class="notification-list" aria-label="Notifications, newest first">
          {entries.map((entry) => {
            const project = projectName(entry.projectId);
            const where = entry.sessionTitle || (entry.sessionId === '' ? 'Server' : 'Untitled session');
            return (
              <li
                class={`notification-row${entry.read ? ' is-read' : ''}`}
                data-tone={KIND_TONES[entry.kind]}
                key={entry.id}
              >
                <button
                  type="button"
                  class="notification-open"
                  onClick={() => void actions.openNotification(entry)}
                  title={
                    entry.sessionId === ''
                      ? entry.title
                      : `Open ${where}${project ? ` in ${project}` : ''}`
                  }
                >
                  {entry.read ? null : <span class="sr-only">Unread. </span>}
                  <span class="notification-head">
                    <span class="notification-kind">{KIND_LABELS[entry.kind]}</span>
                    <span class="notification-title">{entry.title}</span>
                    <span class="notification-time">{relativeTime(entry.at, now)}</span>
                  </span>
                  {entry.body ? <span class="notification-body">{entry.body}</span> : null}
                  <span class="notification-where">
                    {where}
                    {project ? (
                      <>
                        <span class="notification-sep" aria-hidden="true">
                          ·
                        </span>
                        {project}
                      </>
                    ) : null}
                    {entry.source && entry.source !== 'runtime' ? (
                      <>
                        <span class="notification-sep" aria-hidden="true">
                          ·
                        </span>
                        {entry.source}
                      </>
                    ) : null}
                  </span>
                </button>
                {entry.read ? (
                  <span class="notification-read-flag" aria-hidden="true">
                    <CheckIcon size={12} />
                  </span>
                ) : (
                  <button
                    type="button"
                    class="icon-button notification-mark"
                    // The session is in the name because two notifications can
                    // share a title ("Run finished") and a button called "mark that
                    // one read" tells a screen-reader user nothing about which.
                    aria-label={`Mark "${entry.title}" read${
                      entry.sessionTitle ? ` in ${entry.sessionTitle}` : ''
                    }`}
                    title="Mark as read"
                    onClick={() => store.markNotificationRead(entry.id)}
                  >
                    <CheckIcon size={13} />
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </Dialog>
  );
}