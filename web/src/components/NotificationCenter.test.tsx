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
import { NotificationCenter } from './NotificationCenter';

vi.mock('../state/actions', () => ({
  closeNotificationCenter: vi.fn(),
  openNotification: vi.fn(async () => undefined),
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
  vi.mocked(actions.openNotification).mockClear();
  vi.mocked(actions.closeNotificationCenter).mockClear();
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