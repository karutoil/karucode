import { describe, expect, it } from 'vitest';
import {
  STREAM_STEP_MIN_CHARS,
  advanceSplit,
  initialSplit,
  splitStreamingText,
  startsFence,
} from './streamingMarkdown';

/** One settled paragraph: a word-run and the blank line that ends it. */
function block(index: number): string {
  return `Paragraph ${index}. ${'word '.repeat(40)}\n\n`;
}

/** A message with `count` settled paragraphs, each ~200 characters. */
function blocks(count: number): string {
  return Array.from({ length: count }, (_, i) => block(i + 1)).join('');
}

describe('streaming Markdown split', () => {
  it('keeps everything live until a block has settled', () => {
    const split = advanceSplit(initialSplit(), 'one short paragraph');
    expect(split.stable).toBe(0);
    expect(splitStreamingText('one short paragraph', split)).toEqual({
      stable: '',
      live: 'one short paragraph',
    });
  });

  it('settles the blocks behind the newest one once the step is worth it', () => {
    const text = `${blocks(12)}Partial`;
    expect(text.length).toBeGreaterThan(STREAM_STEP_MIN_CHARS);
    const split = advanceSplit(initialSplit(), text);
    const { stable, live } = splitStreamingText(text, split);
    expect(stable.startsWith('Paragraph 1.')).toBe(true);
    expect(stable.endsWith('\n\n')).toBe(true);
    // Only the block still being written is raw.
    expect(live).toBe('Partial');
  });

  it('hands the newest block to the tail', () => {
    const text = `${blocks(12)}still being written`;
    const split = advanceSplit(initialSplit(), text);
    expect(splitStreamingText(text, split).live).toBe('still being written');
  });

  it('does not move the boundary for every new character', () => {
    let split = advanceSplit(initialSplit(), blocks(12));
    const first = split.stable;
    expect(first).toBeGreaterThan(0);
    // One more short block is not enough to justify re-parsing the prefix.
    split = advanceSplit(split, `${blocks(12)}${block(13)}`);
    expect(split.stable).toBe(first);
    // A whole run of blocks eventually is.
    split = advanceSplit(split, `${blocks(40)}`);
    expect(split.stable).toBeGreaterThan(first);
  });

  it('never splits a fenced block across the boundary', () => {
    const body = 'const value = 1;\n'.repeat(300);
    const fence = `\`\`\`ts\n${body}\`\`\``;
    const text = `${blocks(16)}${fence}\n\nafter the fence`;
    const split = advanceSplit(initialSplit(), text);
    const { stable, live } = splitStreamingText(text, split);
    // Whichever side of the boundary it lands on, the fence is one piece.
    expect(stable.includes(fence) || live.startsWith(fence)).toBe(true);
    expect(stable.includes('const value') && live.includes('const value')).toBe(false);
    expect(split.inFence).toBe(false);
  });

  it('reports an unterminated fence, and the tail begins with it', () => {
    const text = `${blocks(16)}\`\`\`ts\n${'const value = 1;\n'.repeat(300)}`;
    const split = advanceSplit(initialSplit(), text);
    const { live } = splitStreamingText(text, split);
    expect(split.inFence).toBe(true);
    expect(startsFence(live)).toBe(true);
  });

  it('does not take a boundary inside a fence', () => {
    // Every blank line here is inside the fence, so nothing has settled.
    const text = `\`\`\`text\n${'line\n\n'.repeat(400)}\`\`\`\n`;
    const split = advanceSplit(initialSplit(), text);
    expect(split.stable).toBe(0);
  });

  it('returns the same state when nothing changed', () => {
    const split = advanceSplit(initialSplit(), blocks(16));
    expect(advanceSplit(split, blocks(16))).toBe(split);
  });

  it('resets when the text is shrunk (a rewind or a replaced answer)', () => {
    const text = blocks(16);
    let split = advanceSplit(initialSplit(), text);
    expect(split.stable).toBeGreaterThan(0);
    split = advanceSplit(split, 'shorter');
    expect(split.stable).toBe(0);
    expect(split.scanned).toBe(0);
    expect(split.inFence).toBe(false);
  });

  it('resets when the already-scanned text was rewritten in place', () => {
    const text = blocks(16);
    let split = advanceSplit(initialSplit(), text);
    const rewritten = `${'x'.repeat(text.length)}still longer than before`;
    split = advanceSplit(split, rewritten);
    expect(split.stable).toBe(0);
    expect(split.scanned).toBe(0);
  });

  it('handles carriage returns and a final line with no newline', () => {
    const text = `${blocks(16).replace(/\n/g, '\r\n')}trailing`;
    const split = advanceSplit(initialSplit(), text);
    expect(split.stable).toBeGreaterThan(0);
    expect(splitStreamingText(text, split).live).toBe('trailing');
  });

  it('recognises a fence opener only at the start of the text', () => {
    expect(startsFence('```rust\nfn one() {}')).toBe(true);
    expect(startsFence('   ~~~\n')).toBe(true);
    expect(startsFence('prose\n```rust\n')).toBe(false);
    expect(startsFence('\n```rust\n')).toBe(false);
  });
});
