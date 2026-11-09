import type { JSX } from 'preact';
import { useRef } from 'preact/hooks';
import { copyText } from '../lib/clipboard';
import { contextMenuProps, Menu, MenuItem, useMenu } from '../lib/menu';
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
 * What `Copy text` puts on the clipboard.
 *
 * The body is what the notification actually says, so it is what gets copied.
 * A plugin notice can arrive with none (`store.recordNotification` keeps the
 * server's `body` as sent), and copying an empty string is a silent nothing
 * followed by a toast that claims success — so the title stands in, which is
 * still the notification's own text. The same fallback shape the sessions rail's
 * `copyTitle` uses for an untitled session.
 */
function notificationText(entry: store.NotificationEntry): string {
  return entry.body.trim() !== '' ? entry.body : entry.title;
}

/**
 * One row in the centre, and its right-click menu.
 *
 * A component of its own because the menu is per row: `useMenu()` is a hook, so
 * one controller per row means one menu can only ever be open at a time here and
 * each row's menu names its own notification.
 *
 * The menu duplicates what the row already shows — the open button and the mark
 * button — and calls the *same* handlers, so there is one code path per action.
 * There is deliberately no per-row dismiss: `store` has no per-notification
 * removal (the log is cleared whole, by `clearNotifications`), and a menu item
 * that pretended otherwise would either be dead or need a second store.
 */
function NotificationRow({
  entry,
  project,
  now,
}: {
  entry: store.NotificationEntry;
  /** The project name for `entry.projectId`, or `null` when it names none. */
  project: string | null;
  now: number;
}): JSX.Element {
  const menu = useMenu();
  const openRef = useRef<HTMLButtonElement>(null);
  const where = entry.sessionTitle || (entry.sessionId === '' ? 'Server' : 'Untitled session');

  const open = (): void => {
    void actions.openNotification(entry);
  };

  const markRead = (): void => {
    store.markNotificationRead(entry.id);
  };

  const copy = async (): Promise<void> => {
    const copied = await copyText(notificationText(entry));
    store.pushToast(
      copied ? 'Notification text copied.' : 'Could not copy the notification text.',
      copied ? 'success' : 'error',
    );
  };

  return (
    <li
      class={`notification-row${entry.read ? ' is-read' : ''}`}
      data-tone={KIND_TONES[entry.kind]}
      // The handler sits on the row, not on a button inside it, and focus returns
      // to the row's own open button: that is the control a keyboard user would
      // have used, so Escape lands the reader where Tab left off.
      {...contextMenuProps(menu, () => true, () => openRef.current)}
    >
      <button
        type="button"
        class="notification-open"
        ref={openRef}
        onClick={open}
        title={
          entry.sessionId === '' ? entry.title : `Open ${where}${project ? ` in ${project}` : ''}`
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
          onClick={markRead}
        >
          <CheckIcon size={13} />
        </button>
      )}

      {/* Pointer-anchored only: this row has no kebab, so the menu is a
        * right-click surface and always floats at the pointer, portalled out of
        * the (scrollable, overflow-hidden) dialog. */}
      <Menu menu={menu} float label={`Notification actions for ${entry.title}`}>
        <MenuItem menu={menu} action={{ label: 'Open', onSelect: open }} />
        {/* Omitted rather than disabled once read, mirroring the row itself,
          * which swaps its mark button for a read flag: a menu that lists an
          * action the row no longer offers is a menu lying about the row. */}
        {entry.read ? null : (
          <MenuItem menu={menu} action={{ label: 'Mark as read', onSelect: markRead }} />
        )}
        <MenuItem menu={menu} action={{ label: 'Copy text', onSelect: () => void copy() }} />
      </Menu>
    </li>
  );
}

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
          {entries.map((entry) => (
            <NotificationRow
              key={entry.id}
              entry={entry}
              project={projectName(entry.projectId)}
              now={now}
            />
          ))}
        </ul>
      )}
    </Dialog>
  );
}
