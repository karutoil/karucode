export type DiffLineKind = 'add' | 'del' | 'context' | 'hunk' | 'meta';

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
  oldNo: number | null;
  newNo: number | null;
}

const HUNK_RE = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

/**
 * Parses a unified diff into display lines. This is presentation only: it never
 * tries to apply the patch. Unknown/header lines are marked as meta and rendered
 * neutrally.
 */
export function parseUnifiedDiff(diff: string, maxLines = 6000): DiffLine[] {
  const lines: DiffLine[] = [];
  let oldNo: number | null = null;
  let newNo: number | null = null;
  let inHunk = false;

  for (const raw of diff.replace(/\r\n/g, '\n').split('\n')) {
    if (lines.length >= maxLines) {
      lines.push({
        kind: 'meta',
        text: `… diff truncated in the viewer (${maxLines} lines shown)`,
        oldNo: null,
        newNo: null,
      });
      break;
    }
    if (raw.startsWith('@@')) {
      const match = HUNK_RE.exec(raw);
      if (match) {
        oldNo = Number(match[1]);
        newNo = Number(match[2]);
      }
      inHunk = true;
      lines.push({ kind: 'hunk', text: raw, oldNo: null, newNo: null });
      continue;
    }
    if (raw.startsWith('diff --git')) {
      inHunk = false;
      oldNo = null;
      newNo = null;
      lines.push({ kind: 'meta', text: raw, oldNo: null, newNo: null });
      continue;
    }
    if (
      raw.startsWith('index ') ||
      (!inHunk && raw.startsWith('--- ')) ||
      (!inHunk && raw.startsWith('+++ ')) ||
      raw.startsWith('new file ') ||
      raw.startsWith('deleted file ') ||
      raw.startsWith('similarity index') ||
      raw.startsWith('rename from') ||
      raw.startsWith('rename to') ||
      raw.startsWith('\\ No newline')
    ) {
      lines.push({ kind: 'meta', text: raw, oldNo: null, newNo: null });
      continue;
    }
    if (raw.startsWith('+')) {
      lines.push({ kind: 'add', text: raw, oldNo: null, newNo });
      if (newNo !== null) newNo += 1;
      continue;
    }
    if (raw.startsWith('-')) {
      lines.push({ kind: 'del', text: raw, oldNo, newNo: null });
      if (oldNo !== null) oldNo += 1;
      continue;
    }
    if (raw.startsWith(' ')) {
      lines.push({ kind: 'context', text: raw, oldNo, newNo });
      if (oldNo !== null) oldNo += 1;
      if (newNo !== null) newNo += 1;
      continue;
    }
    if (raw === '') {
      lines.push({ kind: 'meta', text: '', oldNo: null, newNo: null });
      continue;
    }
    lines.push({ kind: 'meta', text: raw, oldNo: null, newNo: null });
  }

  // Diffs conventionally end with a newline, producing one empty trailing entry.
  while (lines.length > 0 && lines[lines.length - 1]?.text === '') lines.pop();
  return lines;
}