import { describe, expect, it } from 'vitest';
import { preserveIterations } from './agentIterations';

interface Row {
  id: string;
  iterations: number;
  status: string;
}

function row(id: string, iterations: number, status = 'running'): Row {
  return { id, iterations, status };
}

describe('preserveIterations', () => {
  it('keeps the larger count when a fetch returns an older row', () => {
    const merged = preserveIterations([row('a_1', 9)], [row('a_1', 2)]);
    expect(merged[0]?.iterations).toBe(9);
  });

  it('takes the fetched count when it is newer', () => {
    const merged = preserveIterations([row('a_1', 2)], [row('a_1', 9)]);
    expect(merged[0]?.iterations).toBe(9);
  });

  it('follows the fetched membership, not the previous list', () => {
    const merged = preserveIterations(
      [row('a_gone', 7), row('a_kept', 4)],
      [row('a_kept', 1), row('a_new', 0)],
    );
    expect(merged.map((entry) => entry.id)).toEqual(['a_kept', 'a_new']);
    expect(merged.map((entry) => entry.iterations)).toEqual([4, 0]);
  });

  it('is a no-op when nothing was seen live yet', () => {
    const incoming = [row('a_1', 3)];
    expect(preserveIterations([], incoming)).toBe(incoming);
  });
});
