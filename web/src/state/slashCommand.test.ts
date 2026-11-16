import { describe, expect, it } from 'vitest';
import type { SessionDetail, StoredMessage } from '../api/types';
import { detailFixture, storedMessage } from '../test/fixtures';
import { SLASH_ERROR_SUFFIX, SLASH_NAME_PREFIX, slashResultFrom, emptyChatState, seedChatState } from './chat';
import { applyEventToChat } from './eventReducer';

function sessionWith(messages: StoredMessage[]): SessionDetail {
  return {
    ...detailFixture,
    messages,
    runs: [],
    agents: [],
    approvals: [],
    claims: [],
    running: false,
  };
}

function commandRow(id: number, name: string, content: string): StoredMessage {
  return storedMessage({ id, role: 'assistant', name, content });
}

describe('reading a slash result off a stored row', () => {
  it('names the command and whether it was refused', () => {
    expect(slashResultFrom(`${SLASH_NAME_PREFIX}compact`)).toEqual({ command: 'compact', ok: true });
    expect(slashResultFrom(`${SLASH_NAME_PREFIX}compact${SLASH_ERROR_SUFFIX}`)).toEqual({
      command: 'compact',
      ok: false,
    });
  });

  it('ignores every other message name', () => {
    expect(slashResultFrom(null)).toBeNull();
    expect(slashResultFrom(undefined)).toBeNull();
    expect(slashResultFrom('read_file')).toBeNull();
    expect(slashResultFrom('')).toBeNull();
    // A bare prefix has no command in it, so there is nothing to show.
    expect(slashResultFrom(SLASH_NAME_PREFIX)).toBeNull();
  });
});

describe('a command result in the seeded transcript', () => {
  it('becomes its own item, not an assistant message', () => {
    const state = seedChatState(
      sessionWith([
        storedMessage({ id: 1, role: 'user', content: '/help' }),
        commandRow(2, `${SLASH_NAME_PREFIX}help`, '### Slash commands'),
      ]),
    );

    const slash = state.items.find((item) => item.kind === 'slash');
    expect(slash).toMatchObject({
      kind: 'slash',
      id: 'msg:2',
      command: 'help',
      content: '### Slash commands',
      ok: true,
      messageId: 2,
    });
    // It is not also drawn as a model turn.
    expect(state.items.filter((item) => item.kind === 'message' && item.role === 'assistant')).toEqual([]);
  });

  it('keeps a refusal marked as one across a reload', () => {
    const state = seedChatState(
      sessionWith([commandRow(1, `${SLASH_NAME_PREFIX}compact${SLASH_ERROR_SUFFIX}`, 'a run is in flight')]),
    );
    expect(state.items[0]).toMatchObject({ kind: 'slash', command: 'compact', ok: false });
  });
});

describe('the command_result event', () => {
  const base = () => ({ ...emptyChatState('s_1'), sessionId: 's_1' });

  it('appends the answer the row already holds', () => {
    const state = applyEventToChat(base(), {
      seq: 1,
      ts_ms: 1_000,
      session_id: 's_1',
      type: 'command_result',
      command: 'context',
      message_id: 7,
      content: '### Context',
      state: 'ok',
    });

    expect(state.items).toHaveLength(1);
    expect(state.items[0]).toMatchObject({
      kind: 'slash',
      id: 'msg:7',
      command: 'context',
      content: '### Context',
      ok: true,
    });
  });

  it('replaces a row it has already drawn instead of duplicating it', () => {
    const seeded = seedChatState(sessionWith([commandRow(7, `${SLASH_NAME_PREFIX}context`, 'old')]));
    const next = applyEventToChat(seeded, {
      seq: 2,
      ts_ms: 2_000,
      session_id: detailFixture.session.id,
      type: 'command_result',
      command: 'context',
      message_id: 7,
      content: 'new',
      state: 'error',
    });

    expect(next.items).toHaveLength(1);
    expect(next.items[0]).toMatchObject({ kind: 'slash', content: 'new', ok: false });
  });

  it('is ignored for another session', () => {
    const state = applyEventToChat(base(), {
      seq: 3,
      ts_ms: 3_000,
      session_id: 's_other',
      type: 'command_result',
      command: 'help',
      message_id: 9,
      content: 'x',
      state: 'ok',
    });
    expect(state.items).toEqual([]);
  });
});