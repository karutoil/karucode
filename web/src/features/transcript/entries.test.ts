import { describe, expect, it } from 'vitest';
import type { ChatItem, MessageItem } from '../../state/chat';
import { DEFAULT_CHARS_PER_LINE, estimateCharsPerLine, estimateEntryHeight } from './entries';
import type { Entry } from './types';

function message(overrides: Partial<MessageItem>): MessageItem {
  return {
    kind: 'message',
    id: 'm',
    at: 1,
    role: 'assistant',
    content: '',
    reasoning: '',
    streaming: false,
    runId: 'r_x',
    agentId: null,
    ...overrides,
  };
}

function userEntry(content: string): Entry {
  return { kind: 'user', key: 'u', item: message({ role: 'user', content }) };
}

function turnEntry(parts: ChatItem[]): Entry {
  return { kind: 'turn', key: 't', at: 1, parts };
}

describe('estimateCharsPerLine', () => {
  it('turns a measured column into characters per line', () => {
    // A desktop chat column and a 390px phone: the same prose needs more than
    // twice the lines on the phone, which is exactly what the old fixed 96
    // characters could not say (TR-07).
    expect(estimateCharsPerLine(780)).toBe(122);
    expect(estimateCharsPerLine(340)).toBe(53);
    expect(estimateCharsPerLine(340)).toBeLessThan(estimateCharsPerLine(780) / 2);
  });

  it('keeps the historic default when nothing has been measured', () => {
    expect(DEFAULT_CHARS_PER_LINE).toBe(96);
    // jsdom, an embedded renderer, the first paint before layout: all report 0.
    expect(estimateCharsPerLine(0)).toBe(96);
    expect(estimateCharsPerLine(-20)).toBe(96);
    expect(estimateCharsPerLine(Number.NaN)).toBe(96);
    expect(estimateCharsPerLine(Number.POSITIVE_INFINITY)).toBe(96);
  });

  it('never claims a line narrower than a handful of characters', () => {
    // A collapsed panel measures a few pixels; a "line" of one character would
    // make every row four thousand pixels tall.
    expect(estimateCharsPerLine(30)).toBe(24);
    expect(estimateCharsPerLine(1)).toBe(24);
  });
});

describe('estimateEntryHeight', () => {
  it('is unchanged for callers that pass no width', () => {
    // The exact numbers the transcript used before the width was measured: 96
    // characters a line, 18px a line, the same bases per row kind.
    expect(estimateEntryHeight({ kind: 'day', key: 'd', at: 1 })).toBe(48);
    expect(estimateEntryHeight(userEntry('x'.repeat(960)))).toBe(276);
    expect(estimateEntryHeight(turnEntry([message({ content: 'x'.repeat(960) })]))).toBe(316);
    // Explicitly passing the default is the same number, not a different one.
    expect(estimateEntryHeight(userEntry('x'.repeat(960)), DEFAULT_CHARS_PER_LINE)).toBe(276);
  });

  it('claims a taller row for the same text on a phone column', () => {
    const content = 'x'.repeat(960);
    const desktop = estimateEntryHeight(userEntry(content));
    const phone = estimateEntryHeight(userEntry(content), 53);
    // 19 lines instead of 10: the first window's document is no longer a third
    // of its real height, which is the jump the reader used to feel.
    expect(phone).toBe(438);
    expect(phone).toBeGreaterThan(desktop * 1.5);
  });

  it('applies the width to every prose part of a turn', () => {
    const content = 'x'.repeat(960);
    const parts = [message({ content }), message({ content, reasoning: 'x'.repeat(960) })];
    const desktop = estimateEntryHeight(turnEntry(parts));
    const phone = estimateEntryHeight(turnEntry(parts), 53);
    // Both prose parts are re-counted at the narrower column: 10 -> 19 lines for
    // the answer, 20 -> 37 for the answer plus its reasoning.
    expect(phone - desktop).toBe((19 - 10 + (37 - 20)) * 18);
  });

  it('still clamps a guess, whatever the width', () => {
    const huge = turnEntry(Array.from({ length: 40 }, () => message({ content: 'x'.repeat(10_000) })));
    expect(estimateEntryHeight(huge, 24)).toBe(4_000);
  });
});
