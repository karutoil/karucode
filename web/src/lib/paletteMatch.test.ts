import { describe, expect, it } from 'vitest';
import { matchSegments } from './paletteMatch';

/** Join the segments back together: the split may never lose or duplicate text. */
const rejoin = (segments: ReturnType<typeof matchSegments>): string =>
  segments.map((segment) => segment.text).join('');

describe('palette match highlighting', () => {
  it('marks the run the filter matched, and nothing else', () => {
    expect(matchSegments('Hide sessions rail', 'sess')).toEqual([
      { text: 'Hide ', hit: false },
      { text: 'sess', hit: true },
      { text: 'ions rail', hit: false },
    ]);
  });

  it('matches case-insensitively and keeps the original casing on screen', () => {
    // The filter lowercases both halves; the label the reader sees must not be.
    expect(matchSegments('Open Settings', 'settings')).toEqual([
      { text: 'Open ', hit: false },
      { text: 'Settings', hit: true },
    ]);
  });

  it('marks every occurrence, in order', () => {
    expect(matchSegments('a-b-a', 'a')).toEqual([
      { text: 'a', hit: true },
      { text: '-b-', hit: false },
      { text: 'a', hit: true },
    ]);
  });

  it('returns the whole label plain when nothing matches', () => {
    // A row can survive the filter on its *group* or its hint, so "no hit inside
    // the label" is a normal outcome, not an error.
    expect(matchSegments('Open settings', 'plugins')).toEqual([
      { text: 'Open settings', hit: false },
    ]);
  });

  it('treats a blank query as no query', () => {
    expect(matchSegments('Open settings', '   ')).toEqual([{ text: 'Open settings', hit: false }]);
    expect(matchSegments('Open settings', '')).toEqual([{ text: 'Open settings', hit: false }]);
  });

  it('survives an empty label and an awkward query', () => {
    expect(matchSegments('', 'x')).toEqual([{ text: '', hit: false }]);
    // A query of only punctuation is still a query; it must not loop or explode.
    expect(matchSegments('a.b', '.')).toEqual([
      { text: 'a', hit: false },
      { text: '.', hit: true },
      { text: 'b', hit: false },
    ]);
  });

  it('never loses or repeats a character', () => {
    for (const [text, query] of [
      ['Hide sessions rail', 'sess'],
      ['Session titles by model', 'i'],
      ['a-b-a', 'a'],
      ['Open settings', 'zzz'],
      ['', 'q'],
    ] as const) {
      expect(rejoin(matchSegments(text, query)), `${text} / ${query}`).toBe(text);
    }
  });
});
