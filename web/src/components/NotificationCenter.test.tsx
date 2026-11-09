/**
 * The notification centre: the list behind the bell.
 *
 * The centre is the one surface that survives everything else — a dismissed toast, a
 * refused desktop notification, a tab that was in the background — so the tests are
 * about what it can still tell you and what it lets you do with it: read a row, mark
 * one read, mark all read, and get to the session even when it belongs to another
 * project.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/preact';
import type { NotifyPayload, Project } from '../api/types';
import { NOW } from '../test/fixtures';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { copyText } from '../lib/clipboard';
import { NotificationCenter } from './NotificationCenter';

vi.mock('../state/actions', () => ({
  closeNotificationCenter: vi.fn(),
  openNotification: vi.fn(async () => undefined),
}));

// The menu's `Copy text` goes through the same helper the rails use; mocking the
// module (rather than `navigator.clipboard`) asserts the string the row hands it.
vi.mock('../lib/clipboard', () => ({
  copyText: vi.fn(async () => true),
}));

function payload(overrides: Partial<NotifyPayload> = {}): NotifyPayload {
  return {
    id: 'run:r_1',
    kind: 'run_finished',
    title: 'Run finished',
    body: 'All 3 files updated.',
    session_id: 's_1',
    session_title: 'Fix the login redirect',
    project_id: 'p_karucode',
    source: 'runtime',
    sticky: false,
    ...overrides,
  };
}

function project(overrides: Partial<Project> = {}): Project {
  return {
    id: 'p_karucode',
    name: 'KaruCode',
    path: '/home/me/karucode',
    created_at: NOW,
    updated_at: NOW,
    session_count: 2,
    ...overrides,
  };
}

function renderCenter(): ReturnType<typeof render> {
  store.notificationCenterOpen.value = true;
  return render(<NotificationCenter />);
}

beforeEach(() => {
  store.notifications.value = [];
  store.notificationCenterOpen.value = false;
  store.projects.value = [project(), project({ id: 'p_docs', name: 'Docs Site' })];
  store.clock.value = Date.now();
  // `pushToast` arms a real 7s timer; clearing it keeps a copy test from firing
  // into a torn-down environment.
  store.resetToastTimers();
  vi.mocked(actions.openNotification).mockClear();
  vi.mocked(actions.closeNotificationCenter).mockClear();
  vi.mocked(copyText).mockClear();
});

describe('the notification centre', () => {
  it('says so plainly when nothing has arrived', () => {
    renderCenter();
    expect(screen.getByRole('dialog', { name: 'Notifications' })).not.toBeNull();
    expect(screen.getByText('No notifications yet')).not.toBeNull();
    expect(screen.getByText('Nothing yet')).not.toBeNull();
  });

  it('renders nothing at all until it is opened', () => {
    render(<NotificationCenter />);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('lists every notification newest first, with where it came from', () => {
    store.recordNotification(
      payload({ id: 'run:r_1', title: 'Run finished', body: 'All 3 files updated.' }),
      false,
    );
    store.recordNotification(
      payload({
        id: 'approval:a_1',
        kind: 'needs_input',
        title: 'Approval needed',
        body: 'Run cargo test',
        session_title: 'Ship the parser',
        project_id: 'p_docs',
        sticky: true,
      }),
      false,
    );
    // A plugin's name is shown as its source, so "where did this come from" is
    // answerable without opening anything.
    store.recordNotification(
      payload({ id: 'plugin:kc-todo:x', kind: 'info', title: 'Todo list updated', source: 'kc-todo' }),
      false,
    );
    renderCenter();
    const dialog = screen.getByRole('dialog', { name: 'Notifications' });

    const rows = dialog.querySelectorAll('.notification-row');
    expect(rows).toHaveLength(3);
    expect(rows[0]?.textContent).toContain('Todo list updated');
    expect(rows[0]?.textContent).toContain('kc-todo');
    expect(rows[1]?.textContent).toContain('Approval needed');
    expect(rows[2]?.textContent).toContain('Run finished');
    // The kind is spelled out, not left as a colour.
    expect(rows[1]?.textContent).toContain('Needs you');
    expect(rows[2]?.textContent).toContain('Finished');
    // Where: the session, then the project it lives in.
    expect(rows[2]?.textContent).toContain('Fix the login redirect');
    expect(rows[2]?.textContent).toContain('KaruCode');
    expect(rows[1]?.textContent).toContain('Docs Site');
  });

  it('marks the unread ones for screen readers and shows the count', () => {
    store.recordNotification(payload({ id: 'run:r_1' }), false);
    store.recordNotification(payload({ id: 'run:r_2' }), true);
    renderCenter();
    const dialog = screen.getByRole('dialog', { name: 'Notifications' });

    expect(screen.getByText('1 unread, newest first.')).not.toBeNull();
    expect(dialog.textContent).toContain('2 notifications');
    expect(dialog.querySelectorAll('.notification-row.is-read')).toHaveLength(1);
  });

  it('opens the session behind a row, which is a project switch when it is elsewhere', () => {
    store.recordNotification(
      payload({ id: 'run:r_9', session_id: 's_other', project_id: 'p_docs' }),
      false,
    );
    renderCenter();

    fireEvent.click(screen.getByText('Run finished'));
    // The action does the marking and the project switch; this asserts the centre
    // hands it the whole row, including the project the session lives in.
    expect(actions.openNotification).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'run:r_9', sessionId: 's_other', projectId: 'p_docs' }),
    );
  });

  it('marks one row read from its own button', () => {
    store.recordNotification(payload({ id: 'run:r_1' }), false);
    store.recordNotification(
      payload({ id: 'run:r_2', title: 'Run failed', kind: 'error' }),
      false,
    );
    renderCenter();

    fireEvent.click(
      screen.getByRole('button', { name: 'Mark "Run finished" read in Fix the login redirect' }),
    );
    expect(store.unreadTotal.value).toBe(1);
  });

  it('marks everything read, and the button says when there is nothing to mark', () => {
    store.recordNotification(payload({ id: 'run:r_1' }), false);
    store.recordNotification(payload({ id: 'run:r_2' }), false);
    renderCenter();

    const markAll = screen.getByRole('button', { name: /Mark all read/ });
    expect((markAll as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(markAll);
    expect(store.unreadTotal.value).toBe(0);
    expect((screen.getByRole('button', { name: /Mark all read/ }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  it('clears the history, which clears every count derived from it', () => {
    store.recordNotification(payload({ id: 'run:r_1' }), false);
    renderCenter();

    fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(store.notifications.value).toEqual([]);
    expect(store.unreadByProject.value).toEqual({});
    expect(screen.getByText('No notifications yet')).not.toBeNull();
  });

  it('closes through the dialog header, which is how Escape reaches it too', () => {
    store.recordNotification(payload(), false);
    renderCenter();
    const dialog = screen.getByRole('dialog', { name: 'Notifications' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Close dialog' }));
    expect(actions.closeNotificationCenter).toHaveBeenCalledTimes(1);
  });

  it('keeps a notice about no session readable instead of offering a dead link', () => {
    store.recordNotification(
      payload({ id: 'plugin:x:y', session_id: '', session_title: '', project_id: undefined }),
      false,
    );
    renderCenter();
    expect(
      screen.getByRole('dialog', { name: 'Notifications' }).textContent,
    ).toContain('Server');
    // It still counts in the total: the reader has not seen it.
    expect(screen.getByText('1 unread, newest first.')).not.toBeNull();
  });
});

/**
 * The row's right-click menu.
 *
 * The menu is a second road to what the row already shows, never the only one:
 * `Open` is the row's open button's handler, `Mark as read` is the mark button's,
 * and the only thing the menu can do that the row cannot is put the notification's
 * text on the clipboard. That last one is why the tests pin the string handed to
 * `copyText` rather than just that a toast appeared.
 *
 * There is deliberately no per-row dismiss. The store has no per-notification
 * removal — the log is cleared whole by `clearNotifications` — so a menu item
 * claiming one would be either dead or a second store.
 */
describe('the notification row menu', () => {
  /** Right-click the row whose title contains `title`; return the menu. */
  function openMenuOn(title: string): HTMLElement {
    const row = [...document.querySelectorAll('.notification-row')].find((node) =>
      node.textContent?.includes(title),
    ) as HTMLElement | undefined;
    expect(row, `row for "${title}"`).not.toBeUndefined();
    // Suppressing the native menu is the whole point of offering our own.
    expect(fireEvent.contextMenu(row!, { clientX: 60, clientY: 200 })).toBe(false);
    return screen.getByRole('menu', { name: `Notification actions for ${title}` });
  }

  it('opens on right-click with the row’s own actions, portalled out of the list', () => {
    store.recordNotification(payload({ id: 'run:r_1' }), false);
    renderCenter();

    const menu = openMenuOn('Run finished');
    // The float menu portals to <body>, so the list's own scroll cannot clip it.
    expect(menu.closest('.menu-layer')?.parentElement).toBe(document.body);
    expect(
      within(menu).getAllByRole('menuitem').map((item) => item.textContent),
    ).toEqual(['Open', 'Mark as read', 'Copy text']);
    // The menu took nothing away: both visible controls are still on the row.
    expect(
      screen.getByRole('button', { name: 'Mark "Run finished" read in Fix the login redirect' }),
    ).not.toBeNull();
    expect(document.querySelector('.notification-row .notification-open')).not.toBeNull();
    expect(document.activeElement).toBe(within(menu).getByRole('menuitem', { name: 'Open' }));
  });

  it('opens the session from the menu through the row’s own handler', () => {
    store.recordNotification(
      payload({ id: 'run:r_9', session_id: 's_other', project_id: 'p_docs' }),
      false,
    );
    renderCenter();

    fireEvent.click(within(openMenuOn('Run finished')).getByRole('menuitem', { name: 'Open' }));
    expect(actions.openNotification).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'run:r_9', sessionId: 's_other', projectId: 'p_docs' }),
    );
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('marks the row read through the same store action its button calls', () => {
    store.recordNotification(payload({ id: 'run:r_1' }), false);
    store.recordNotification(payload({ id: 'run:r_2', title: 'Run failed', kind: 'error' }), false);
    renderCenter();
    const mark = vi.spyOn(store, 'markNotificationRead');

    fireEvent.click(
      within(openMenuOn('Run finished')).getByRole('menuitem', { name: 'Mark as read' }),
    );
    // The same call the mark button's test above makes, with the same effect on
    // every count derived from the log.
    expect(mark).toHaveBeenCalledWith('run:r_1');
    expect(store.unreadTotal.value).toBe(1);
    mark.mockRestore();
  });

  it('copies the notification’s body text and says so', async () => {
    store.recordNotification(payload({ id: 'run:r_1', body: 'All 3 files updated.' }), false);
    renderCenter();

    fireEvent.click(within(openMenuOn('Run finished')).getByRole('menuitem', { name: 'Copy text' }));
    await vi.waitFor(() => expect(copyText).toHaveBeenCalledWith('All 3 files updated.'));
    await vi.waitFor(() =>
      expect(store.toasts.value.map((toast) => toast.message)).toContain(
        'Notification text copied.',
      ),
    );
  });

  it('copies the title when a notice arrived with no body', async () => {
    // A plugin notice can carry no body; copying an empty string would be a
    // silent nothing behind a toast that claims success.
    store.recordNotification(
      payload({ id: 'plugin:kc-todo:x', title: 'Todo list updated', body: '' }),
      false,
    );
    renderCenter();

    fireEvent.click(
      within(openMenuOn('Todo list updated')).getByRole('menuitem', { name: 'Copy text' }),
    );
    await vi.waitFor(() => expect(copyText).toHaveBeenCalledWith('Todo list updated'));
  });

  it('reports a refused clipboard instead of claiming a copy', async () => {
    vi.mocked(copyText).mockResolvedValueOnce(false);
    store.recordNotification(payload({ id: 'run:r_1' }), false);
    renderCenter();

    fireEvent.click(within(openMenuOn('Run finished')).getByRole('menuitem', { name: 'Copy text' }));
    await vi.waitFor(() =>
      expect(store.toasts.value.map((toast) => [toast.message, toast.tone])).toContainEqual([
        'Could not copy the notification text.',
        'error',
      ]),
    );
  });

  it('has no Mark as read on a row that has been read', () => {
    store.recordNotification(payload({ id: 'run:r_1' }), true);
    renderCenter();

    const menu = openMenuOn('Run finished');
    // The row itself swaps its mark button for a read flag; the menu must not
    // keep offering an action the row no longer shows.
    expect(
      within(menu)
        .queryAllByRole('menuitem', { name: 'Mark as read' })
        .filter((item) => !(item as HTMLButtonElement).disabled),
    ).toEqual([]);
    expect(within(menu).queryByRole('menuitem', { name: 'Mark as read' })).toBeNull();
    expect(
      within(menu).getAllByRole('menuitem').map((item) => item.textContent),
    ).toEqual(['Open', 'Copy text']);
  });

  it('closes on Escape and gives focus back to the row’s open button', () => {
    store.recordNotification(payload({ id: 'run:r_1' }), false);
    renderCenter();
    const menu = openMenuOn('Run finished');
    const open = document.querySelector('.notification-row .notification-open') as HTMLElement;

    fireEvent.keyDown(within(menu).getByRole('menuitem', { name: 'Copy text' }), {
      key: 'Escape',
    });
    expect(screen.queryByRole('menu')).toBeNull();
    // Escape inside the menu is the menu's: the centre stays open behind it.
    expect(actions.closeNotificationCenter).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog', { name: 'Notifications' })).not.toBeNull();
    expect(document.activeElement).toBe(open);
  });

  it('names each row’s menu after that row, not after the list', () => {
    store.recordNotification(payload({ id: 'run:r_1', title: 'Run finished' }), false);
    store.recordNotification(payload({ id: 'run:r_2', title: 'Run failed', kind: 'error' }), false);
    renderCenter();

    openMenuOn('Run failed');
    expect(
      screen.getByRole('menu', { name: 'Notification actions for Run failed' }),
    ).not.toBeNull();
    expect(screen.queryByRole('menu', { name: 'Notification actions for Run finished' })).toBeNull();
  });
});
