import { describe, expect, it } from 'vitest';
import {
  activeMention,
  formatMention,
  indexFromTree,
  insertMention,
  isSubsequence,
  needsQuoting,
  rankMentions,
  scoreMention,
  type MentionCandidate,
} from './mentions';
import type { WorkspaceTree } from '../api/types';

const tree: WorkspaceTree = {
  path: '.',
  entries: [
    { name: 'src', path: 'src', kind: 'dir', size: 0 },
    { name: 'main.ts', path: 'src/main.ts', kind: 'file', size: 812 },
    { name: 'main.rs', path: 'crates/kc-core/src/main.rs', kind: 'file', size: 120 },
    { name: 'README.md', path: 'README.md', kind: 'file', size: 2100 },
    { name: 'node_modules', path: 'node_modules', kind: 'dir', size: 0 },
    { name: 'index.js', path: 'node_modules/lib/index.js', kind: 'file', size: 10 },
    { name: 'plan', path: 'docs/plan.md', kind: 'file', size: 40 },
  ],
};

describe('indexFromTree', () => {
  it('flattens the tree and drops noise directories', () => {
    const index = indexFromTree(tree);
    expect(index.map((entry) => entry.path)).toEqual([
      'src',
      'src/main.ts',
      'crates/kc-core/src/main.rs',
      'README.md',
      'docs/plan.md',
    ]);
  });

  it('keeps kind and size, and tolerates a malformed entry', () => {
    const index = indexFromTree({
      path: '.',
      entries: [
        { name: 'a.ts', path: 'a.ts', kind: 'file', size: 7 },
        { name: 'dir', path: 'dir', kind: 'dir', size: 0 },
        { name: '', path: '', kind: 'file', size: 1 },
      ],
    });
    expect(index).toEqual([
      { path: 'a.ts', kind: 'file', size: 7 },
      { path: 'dir', kind: 'dir', size: 0 },
    ]);
  });
});

describe('rankMentions', () => {
  const index = indexFromTree(tree);

  it('ranks a basename prefix above a path substring', () => {
    expect(rankMentions(index, 'main')[0]?.path).toBe('src/main.ts');
  });

  it('prefers an exact basename over other matches', () => {
    expect(rankMentions(index, 'README.md')[0]?.path).toBe('README.md');
  });

  it('matches a fuzzy subsequence of the path', () => {
    expect(rankMentions(index, 'kc-core').map((entry) => entry.path)).toContain(
      'crates/kc-core/src/main.rs',
    );
  });

  it('prefers files and shallow paths for an empty query', () => {
    const paths = rankMentions(index, '').map((entry) => entry.path);
    expect(paths[0]).toBe('README.md');
    expect(paths).toContain('src');
    expect(paths).not.toContain('node_modules');
  });

  it('never suggests an ignored path', () => {
    expect(rankMentions(index, 'index.js')).toEqual([]);
  });

  it('scores nothing outside the workspace index', () => {
    expect(rankMentions(index, 'nothing-like-this')).toEqual([]);
  });

  it('respects the limit and stays sorted', () => {
    const limited = rankMentions(index, 'src', 2);
    expect(limited).toHaveLength(2);
    expect(scoreMention(limited[0] as MentionCandidate, 'src')).toBeGreaterThanOrEqual(
      scoreMention(limited[1] as MentionCandidate, 'src'),
    );
  });

  it('is case-insensitive and accepts backslashes', () => {
    expect(rankMentions(index, 'MAIN.TS')[0]?.path).toBe('src/main.ts');
    expect(rankMentions(index, 'src\\main')[0]?.path).toBe('src/main.ts');
  });
});

describe('isSubsequence', () => {
  it('matches in-order characters only', () => {
    expect(isSubsequence('abc', 'axbxc')).toBe(true);
    expect(isSubsequence('cba', 'abc')).toBe(false);
    expect(isSubsequence('', 'anything')).toBe(true);
  });
});

describe('activeMention', () => {
  it('finds the token under the caret', () => {
    const text = 'look at @src/ma';
    expect(activeMention(text, text.length)).toEqual({
      start: 8,
      end: 15,
      query: 'src/ma',
      quoted: false,
    });
  });

  it('does not treat an email address as a mention', () => {
    const text = 'mail me@example.com';
    expect(activeMention(text, text.length)).toBeNull();
  });

  it('closes the token at whitespace', () => {
    const text = '@src/main.ts then explain';
    expect(activeMention(text, 5)).toEqual({ start: 0, end: 12, query: 'src/', quoted: false });
  });

  it('supports quoted paths with spaces', () => {
    const text = 'read @"docs/my notes.md" please';
    const caret = 'read @"docs/my notes.md"'.length;
    expect(activeMention(text, caret)).toEqual({
      start: 5,
      end: 24,
      query: 'docs/my notes.md',
      quoted: true,
    });
  });

  it('is closed when the caret sits beyond the token', () => {
    expect(activeMention('@src/main.ts more', 17)).toBeNull();
  });

  it('ignores an @ on the previous line', () => {
    const text = '@src/main.ts\nplain text';
    expect(activeMention(text, text.length)).toBeNull();
  });

  it('returns null when a line has no @', () => {
    expect(activeMention('no mention here', 4)).toBeNull();
  });

  it('stays sub-millisecond on a 2MB single-line paste', () => {
    // A pasted file is one enormous "word". The token was closed by scanning
    // forward one character at a time to the next whitespace, so the composer
    // walked the whole paste on *every keystroke*: 2MB here, and a real paste of
    // a minified bundle is 10MB. The scan is bounded now, so the caret in the
    // middle of the paste costs the same as the caret at its end.
    const paste = `@src/main${'x'.repeat(2_000_000)}`;
    const medianMs = (call: () => void): number => {
      const samples: number[] = [];
      for (let index = 0; index < 5; index += 1) {
        const started = performance.now();
        call();
        samples.push(performance.now() - started);
      }
      samples.sort((a, b) => a - b);
      // The median, not the mean: one sample can be a GC pause or a scheduler
      // stall, and this must measure the scan rather than the machine.
      return samples[Math.floor(samples.length / 2)] as number;
    };
    // Both outcomes are "no mention": the far caret is more than a token's reach
    // past the `@`, and the near one starts a token that never ends.
    expect(activeMention(paste, paste.length)).toBeNull();
    expect(activeMention(paste, 4)).toBeNull();
    // An O(n) scan over 2MB takes ~50-200ms; bounded O(1) stays <1ms standalone
    // and well under 10ms even under heavy parallel Vitest CPU contention.
    expect(medianMs(() => activeMention(paste, paste.length))).toBeLessThan(10);
    expect(medianMs(() => activeMention(paste, 4))).toBeLessThan(10);
  });

  it('still completes a mention typed after a huge paste', () => {
    // Bounding the scan must not cost the ordinary case: a `@` typed at the end
    // of a long line still opens the menu, and its token still ends where the
    // text does.
    const text = `${'x'.repeat(2_000_000)} @src/ma`;
    expect(activeMention(text, text.length)).toEqual({
      start: 2_000_001,
      end: text.length,
      query: 'src/ma',
      quoted: false,
    });
  });
});

describe('insertMention', () => {
  it('replaces the token and leaves the caret after the path', () => {
    const text = 'check @src/ma today';
    const mention = activeMention(text, 13);
    expect(mention).not.toBeNull();
    const result = insertMention(text, mention!, 'src/main.ts');
    expect(result.text).toBe('check @src/main.ts  today');
    expect(result.caret).toBe('check @src/main.ts '.length);
  });

  it('quotes paths that contain spaces', () => {
    const text = '@pla';
    const mention = activeMention(text, text.length);
    const result = insertMention(text, mention!, 'docs/my plan.md');
    expect(result.text).toBe('@"docs/my plan.md" ');
    expect(result.caret).toBe(result.text.length);
  });

  it('keeps the text after the caret', () => {
    const text = 'see @ma and fix it';
    const mention = activeMention(text, 7);
    const result = insertMention(text, mention!, 'src/main.ts');
    expect(result.text).toBe('see @src/main.ts  and fix it');
  });
});

describe('formatMention / needsQuoting', () => {
  it('quotes only when needed', () => {
    expect(needsQuoting('src/main.ts')).toBe(false);
    expect(needsQuoting('my file.ts')).toBe(true);
    expect(formatMention('src/main.ts')).toBe('@src/main.ts');
    expect(formatMention('my file.ts')).toBe('@"my file.ts"');
  });
});
