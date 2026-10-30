import { describe, expect, it } from 'vitest';
import type { ImageAttachment, SessionDetail, StoredMessage, ToolCall } from '../api/types';
import { detailFixture, storedMessage } from '../test/fixtures';
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
