import { describe, expect, it } from 'vitest';
import type { ImageAttachment, SessionDetail, StoredMessage, ToolCall } from '../api/types';
import { detailFixture, queuedFixture, storedMessage } from '../test/fixtures';
import { seedChatState } from './chat';

const shot: ImageAttachment = {
  media_type: 'image/png',
  data: 'iVBORw0KGgo=',
  bytes: 8,
  name: 'shot.png',
};

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

const call: ToolCall = { id: 'call_1', name: 'read_file', arguments: '{"path":"shot.png"}' };

function assistantTurn(): StoredMessage {
  return storedMessage({ id: 1, role: 'assistant', tool_calls: [call] });
}

describe('seedChatState', () => {
  it('carries both queues off the snapshot, steers first', () => {
    // Steers land before follow-ups (a steer joins the turn in flight, a
    // follow-up gets its own run), so the pending list is in landing order.
    const state = seedChatState({
      ...detailFixture,
      running: true,
      queued_steers: [queuedFixture({ id: 'steer_1', kind: 'steer' })],
      queued_follow_ups: [
        queuedFixture({ id: 'followup_1', kind: 'follow_up', draft: 'then run the tests' }),
      ],
    });
    expect(state.queued.map((entry) => entry.id)).toEqual(['steer_1', 'followup_1']);
  });

  it('has an empty queue when the snapshot carries none', () => {
    expect(seedChatState({ ...detailFixture, running: false }).queued).toEqual([]);
  });

  it('carries the stored tool images onto the card so a reload still previews them', () => {
    const toolRow: StoredMessage = {
      ...storedMessage({
        id: 2,
        role: 'tool',
        tool_call_id: 'call_1',
        name: 'read_file',
        content: 'shot.png — image/png (8 B)',
      }),
      images: [shot],
    };
    const state = seedChatState(sessionWith([assistantTurn(), toolRow]));

    const tool = state.items.find((item) => item.kind === 'tool');
    expect(tool?.kind === 'tool' ? tool.images : []).toEqual([shot]);
    expect(tool?.kind === 'tool' ? tool.output : '').toContain('image/png');
  });

  it('leaves images out for a text-only tool result', () => {
    const toolRow = storedMessage({
      id: 2,
      role: 'tool',
      tool_call_id: 'call_1',
      name: 'read_file',
      content: 'a.txt — lines 1-1 of 1',
    });
    const state = seedChatState(sessionWith([assistantTurn(), toolRow]));

    const tool = state.items.find((item) => item.kind === 'tool');
    expect(tool?.kind === 'tool' ? tool.images : undefined).toEqual([]);
  });
});
