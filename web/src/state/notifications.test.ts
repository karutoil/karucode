/**
 * The notification log: one list behind four counters.
 *
 * The important property is that there is exactly *one* source of truth. The dot on
 * a session row, the count on a project tile, the tab title and the notification
 * centre all read `store.notifications`, so these tests are mostly about the
 * derivation being right — and about the log surviving a reload without trusting
 * whatever `localStorage` hands back.
 */

import { beforeEach, describe, expect, it } from 'vitest';
import type { NotifyPayload } from '../api/types';
import * as store from './store';

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

beforeEach(() => {
  store.notifications.value = [];
  store.notificationCenterOpen.value = false;
  localStorage.clear();
});

describe('recording a notification', () => {
  it('keeps the newest first and derives one unread count per session', () => {
    store.recordNotification(payload({ id: 'run:r_1' }), false);
    store.recordNotification(payload({ id: 'run:r_2' }), false);
    store.recordNotification(payload({ id: 'run:r_3', session_id: 's_2' }), false);

    expect(store.notifications.value.map((entry) => entry.id)).toEqual([
      'run:r_3',
      'run:r_2',
      'run:r_1',
    ]);
    expect(store.unread.value).toEqual({ s_1: 2, s_2: 1 });
    expect(store.unreadTotal.value).toBe(3);
    expect(store.unreadByProject.value).toEqual({ p_karucode: 3 });
  });

  it('dedupes on the server id, so a replayed frame is not a second notification', () => {
    store.recordNotification(payload({ id: 'run:r_1' }), false);
    store.recordNotification(payload({ id: 'run:r_1' }), false);
    expect(store.notifications.value).toHaveLength(1);
    expect(store.unreadTotal.value).toBe(1);
  });

  it('records a notification about the open session as already read', () => {
    // The toast was on screen, so it must not also raise a badge - but it is still
    // in the history, because the centre is a log rather than a dismissal queue.
    store.recordNotification(payload(), true);
    expect(store.notifications.value).toHaveLength(1);
    expect(store.notifications.value[0]?.read).toBe(true);
    expect(store.unreadTotal.value).toBe(0);
    expect(store.unread.value).toEqual({});
  });

  it('counts an unattributable notice in the total but on no tile', () => {
    // A plugin notice about no session at all: there is nowhere to hang it.
    store.recordNotification(
      payload({ id: 'plugin:x:hello', session_id: '', session_title: '', project_id: undefined }),
      false,
    );
    expect(store.unreadTotal.value).toBe(1);
    expect(store.unread.value).toEqual({});
    expect(store.unreadByProject.value).toEqual({});
  });

  it('ignores a payload with no id instead of storing a row nothing can dedupe', () => {
    store.recordNotification(payload({ id: '' }), false);
    expect(store.notifications.value).toEqual([]);
  });

  it('coerces an unknown kind to info rather than dropping it', () => {
    // A newer server may learn kinds before this browser does.
    store.recordNotification(payload({ kind: 'brand_new' as NotifyPayload['kind'] }), false);
    expect(store.notifications.value[0]?.kind).toBe('info');
  });

  it('keeps the log bounded, newest first', () => {
    for (let index = 0; index < store.NOTIFICATION_LIMIT + 12; index += 1) {
      store.recordNotification(payload({ id: `run:r_${index}` }), false);
    }
    expect(store.notifications.value).toHaveLength(store.NOTIFICATION_LIMIT);
    expect(store.notifications.value[0]?.id).toBe(
      `run:r_${store.NOTIFICATION_LIMIT + 11}`,
    );
  });
});

describe('reading and clearing', () => {
  beforeEach(() => {
    store.recordNotification(payload({ id: 'run:r_1' }), false);
    store.recordNotification(payload({ id: 'run:r_2' }), false);
    store.recordNotification(payload({ id: 'run:r_3', session_id: 's_2', project_id: 'p_docs' }), false);
  });

  it('clears one session without touching the others', () => {
    store.clearUnread('s_1');
    expect(store.unread.value).toEqual({ s_2: 1 });
    expect(store.unreadByProject.value).toEqual({ p_docs: 1 });
    // Read, still listed: the history is what the centre shows.
    expect(store.notifications.value).toHaveLength(3);
    expect(store.notifications.value.filter((entry) => entry.read)).toHaveLength(2);
  });

  it('marks a single entry read by id', () => {
    store.markNotificationRead('run:r_2');
    expect(store.unread.value).toEqual({ s_1: 1, s_2: 1 });
  });

  it('marks one project read, leaving the other project alone', () => {
    store.markProjectNotificationsRead('p_karucode');
    expect(store.unread.value).toEqual({ s_2: 1 });
    expect(store.unreadByProject.value).toEqual({ p_docs: 1 });
  });

  it('marks everything read at once', () => {
    store.markAllNotificationsRead();
    expect(store.unreadTotal.value).toBe(0);
    expect(store.notifications.value.every((entry) => entry.read)).toBe(true);
  });

  it('forgets a deleted session entirely instead of leaving a permanent badge', () => {
    store.clearUnread(null);
    store.forgetSessionNotifications('s_2');
    expect(store.notifications.value.map((entry) => entry.id)).toEqual(['run:r_2', 'run:r_1']);
    expect(store.unreadByProject.value).toEqual({ p_karucode: 2 });
  });

  it('clears the history and every count derived from it', () => {
    store.clearNotifications();
    expect(store.notifications.value).toEqual([]);
    expect(store.unreadTotal.value).toBe(0);
    expect(store.unreadByProject.value).toEqual({});
  });
});

describe('persistence', () => {
  it('writes the log so a reload can show what arrived while the tab was closed', () => {
    store.recordNotification(payload(), false);
    const raw = localStorage.getItem('karucode.notifications');
    expect(raw).not.toBeNull();
    const parsed = JSON.parse(raw ?? '[]') as unknown[];
    expect(parsed).toHaveLength(1);
    expect(store.readNotificationLog()).toHaveLength(1);
    expect(store.readNotificationLog()[0]).toMatchObject({
      id: 'run:r_1',
      sessionId: 's_1',
      projectId: 'p_karucode',
      read: false,
    });
  });

  it('records the read flag when the write lands', () => {
    store.recordNotification(payload(), false);
    store.markNotificationRead('run:r_1');
    expect(JSON.parse(localStorage.getItem('karucode.notifications') ?? '[]')[0].read).toBe(true);
  });

  it('drops a malformed log rather than rendering undefined', () => {
    localStorage.setItem('karucode.notifications', '{not json');
    expect(store.readNotificationLog()).toEqual([]);

    localStorage.setItem('karucode.notifications', JSON.stringify({ nope: true }));
    expect(store.readNotificationLog()).toEqual([]);

    localStorage.setItem(
      'karucode.notifications',
      JSON.stringify([
        { id: 'a', kind: 'run_finished', title: 'ok', at: Date.now(), read: true },
        { id: 'b', kind: 'made_up_kind', title: 'dropped', at: Date.now() },
        { kind: 'info', title: 'no id', at: Date.now() },
        'not an object',
      ]),
    );
    const loaded = store.readNotificationLog();
    expect(loaded.map((entry) => entry.id)).toEqual(['a']);
    // Everything absent is normalised rather than left undefined.
    expect(loaded[0]).toMatchObject({ body: '', sessionId: '', projectId: null, read: true });
  });

  it('drops entries older than the retention window', () => {
    const old = Date.now() - 8 * 86_400_000;
    localStorage.setItem(
      'karucode.notifications',
      JSON.stringify([
        { id: 'old', kind: 'info', title: 'stale', at: old, read: false },
        { id: 'new', kind: 'info', title: 'fresh', at: Date.now(), read: false },
      ]),
    );
    expect(store.readNotificationLog().map((entry) => entry.id)).toEqual(['new']);
  });

  it('caps what it loads, even if the stored array grew elsewhere', () => {
    const rows = Array.from({ length: store.NOTIFICATION_LIMIT + 5 }, (_value, index) => ({
      id: `run:r_${index}`,
      kind: 'info',
      title: 't',
      at: Date.now(),
      read: false,
    }));
    localStorage.setItem('karucode.notifications', JSON.stringify(rows));
    expect(store.readNotificationLog()).toHaveLength(store.NOTIFICATION_LIMIT);
  });
});