import { describe, expect, it } from 'vitest';
import { parseUnifiedDiff } from './diff';

const DIFF = `diff --git a/src/main.rs b/src/main.rs
index 1111111..2222222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,3 @@ fn main() {
 fn main() {
-    println!("old");
+    println!("new");
 }
`;

describe('parseUnifiedDiff', () => {
  it('classifies headers, hunks, additions, deletions and context', () => {
    const lines = parseUnifiedDiff(DIFF);
    expect(lines.map((line) => line.kind)).toEqual([
      'meta',
      'meta',
      'meta',
      'meta',
      'hunk',
      'context',
      'del',
      'add',
      'context',
    ]);
    const add = lines.find((line) => line.kind === 'add');
    expect(add?.text).toContain('println!("new")');
    expect(add?.newNo).toBe(2);
    const del = lines.find((line) => line.kind === 'del');
    expect(del?.oldNo).toBe(2);
    const context = lines.filter((line) => line.kind === 'context');
    expect(context[0]?.oldNo).toBe(1);
    expect(context[1]?.oldNo).toBe(3);
  });

  it('does not mistake hunk content that looks like a file header', () => {
    const diff = `diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,2 +1,2 @@
--- removed line
+++ added line
 `;
    const lines = parseUnifiedDiff(diff);
    expect(lines.find((line) => line.kind === 'del')?.text).toBe('--- removed line');
    expect(lines.find((line) => line.kind === 'add')?.text).toBe('+++ added line');
  });

  it('handles an empty diff and caps very large diffs', () => {
    expect(parseUnifiedDiff('')).toEqual([]);
    const huge = `${DIFF}\n${'+x\n'.repeat(20)}`;
    const lines = parseUnifiedDiff(huge, 12);
    expect(lines.length).toBe(13);
    expect(lines[lines.length - 1]?.text).toContain('truncated');
  });
});