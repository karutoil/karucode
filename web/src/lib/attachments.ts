/**
 * Local-file attachments for the composer.
 *
 * Two shapes, decided per file:
 *
 * - **inline** — small text files are read in the browser and pasted into the
 *   message, so the model sees the content with no extra tool round-trip;
 * - **upload** — everything else (binary, large, unknown) goes to
 *   `POST /api/workspace/upload` and is referenced by its workspace path,
 *   which the agent can open with its own file tools.
 *
 * Nothing here touches the network; the component owns that.
 */

import { formatBytes } from './format';
import type { QueuedAttachment } from '../api/types';

export type AttachmentMode = 'inline' | 'upload';

/** Text files up to this size are inlined into the message. */
export const INLINE_MAX_BYTES = 64 * 1024;
/**
 * Refuse larger files client-side; mirrors `MAX_UPLOAD_BYTES` in
 * `crates/kc-server/src/routes/workspace.rs`, which is the authority.
 */
export const UPLOAD_MAX_BYTES = 8 * 1024 * 1024;

/** Extensions we treat as text even when the browser reports no MIME type. */
export const TEXT_EXTENSIONS: readonly string[] = [
  'txt', 'md', 'markdown', 'mdx', 'rst', 'adoc', 'log',
  'json', 'jsonc', 'json5', 'yaml', 'yml', 'toml', 'ini', 'cfg', 'conf', 'env',
  'csv', 'tsv', 'xml', 'html', 'htm', 'css', 'scss', 'sass', 'less',
  'js', 'jsx', 'mjs', 'cjs', 'ts', 'tsx', 'mts', 'cts', 'vue', 'svelte', 'astro',
  'py', 'pyi', 'rb', 'go', 'rs', 'java', 'kt', 'kts', 'swift', 'c', 'h', 'cc', 'cpp',
  'hpp', 'cs', 'm', 'mm', 'php', 'pl', 'r', 'jl', 'lua', 'ex', 'exs', 'erl', 'hs',
  'scala', 'clj', 'cljs', 'dart', 'zig', 'sql', 'graphql', 'gql', 'proto',
  'sh', 'bash', 'zsh', 'fish', 'ps1', 'bat', 'cmd',
  'diff', 'patch', 'gitignore', 'dockerignore', 'editorconfig', 'lock',
];

export function extensionOf(name: string): string {
  const base = name.replace(/^.*[\\/]/, '');
  const dot = base.lastIndexOf('.');
  if (dot <= 0) return base.toLowerCase() === 'dockerfile' ? 'dockerfile' : '';
  return base.slice(dot + 1).toLowerCase();
}

/** `inline` when the file is plausibly small plain text, `upload` otherwise. */
export function classifyByName(
  name: string,
  size: number,
  type: string = '',
): { mode: AttachmentMode; reason: string } {
  if (size > UPLOAD_MAX_BYTES) {
    return { mode: 'upload', reason: `Larger than ${formatBytes(UPLOAD_MAX_BYTES)}` };
  }
  const mime = type.toLowerCase();
  const extension = extensionOf(name);
  const mimeIsText = mime.startsWith('text/') || mime === 'application/json' || mime === '';
  const extensionIsText = TEXT_EXTENSIONS.includes(extension);

  if (!mimeIsText && !extensionIsText) {
    return { mode: 'upload', reason: 'Binary file' };
  }
  if (size > INLINE_MAX_BYTES) {
    return { mode: 'upload', reason: `Too large to inline (${formatBytes(size)})` };
  }
  return { mode: 'inline', reason: 'Small text file' };
}

/** A NUL byte in the first chunk is the classic "this is not text" tell. */
export function looksBinary(bytes: Uint8Array): boolean {
  const sample = bytes.subarray(0, 4096);
  for (const byte of sample) {
    if (byte === 0) return true;
  }
  return false;
}

/** The subset of the DOM `File` API this module needs (keeps tests light). */
export interface FileLike {
  name: string;
  size: number;
  type?: string;
  slice?: (start?: number, end?: number) => Blob;
}

/** Decide the pipeline for a real file, sniffing the head of the content. */
export async function classifyFile(file: FileLike): Promise<{ mode: AttachmentMode; reason: string }> {
  const guess = classifyByName(file.name, file.size, file.type ?? '');
  if (guess.mode === 'upload') return guess;
  try {
    const head = await readBytes(file, 0, Math.min(file.size, 4096));
    if (looksBinary(head)) return { mode: 'upload', reason: 'Binary file' };
  } catch {
    return { mode: 'upload', reason: 'Could not read the file' };
  }
  return guess;
}

async function readBlob(blob: Blob): Promise<ArrayBuffer> {
  if (typeof blob.arrayBuffer === 'function') return blob.arrayBuffer();
  return new Promise<ArrayBuffer>((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error('Could not read the file'));
    reader.onload = () => resolve(reader.result as ArrayBuffer);
    reader.readAsArrayBuffer(blob);
  });
}

/** Read a byte range of the file (`end` exclusive). */
export async function readBytes(file: FileLike, start = 0, end?: number): Promise<Uint8Array> {
  const slice = file.slice ? file.slice(start, end) : (file as unknown as Blob);
  return new Uint8Array(await readBlob(slice));
}

export async function readText(file: FileLike): Promise<string> {
  return new TextDecoder().decode(await readBytes(file));
}

export type AttachmentStatus = 'reading' | 'uploading' | 'ready' | 'error';

/** One chip in the composer. */
export interface Attachment {
  id: string;
  /** Original file name, as picked or dropped. */
  name: string;
  size: number;
  mode: AttachmentMode;
  /** Workspace path to reference; for uploads this is the server's answer. */
  path: string;
  /** Inline text; only for `mode === 'inline'` attachments that are ready. */
  content?: string;
  status: AttachmentStatus;
  error?: string;
}

let sequence = 0;

export function newAttachmentId(name: string): string {
  sequence += 1;
  return `${name}:${Date.now().toString(36)}:${sequence}`;
}

/** Chip subtitle: what will happen to this file, or why it failed. */
export function attachmentSummary(attachment: Attachment): string {
  const size = attachment.size > 0 ? formatBytes(attachment.size) : '';
  if (attachment.status === 'reading') return 'Reading…';
  if (attachment.status === 'uploading') return `Uploading${size ? ` · ${size}` : ''}…`;
  if (attachment.status === 'error') return attachment.error ?? 'Failed';
  const mode = attachment.mode === 'inline' ? 'Inlined' : 'Uploaded';
  return size ? `${mode} · ${size}` : mode;
}

export function canSendAttachments(attachments: readonly Attachment[]): boolean {
  return attachments.every((attachment) => attachment.status === 'ready');
}

export function removeAttachment(attachments: readonly Attachment[], id: string): Attachment[] {
  return attachments.filter((attachment) => attachment.id !== id);
}

/** Longest fence in the content, so nested ``` blocks cannot break the file. */
function fenceFor(content: string): string {
  const longest = content.match(/`{3,}/g)?.reduce((max, run) => Math.max(max, run.length), 0) ?? 0;
  return '`'.repeat(Math.max(3, longest + 1));
}

function languageOf(path: string): string {
  const extension = extensionOf(path);
  if (extension === '' || extension.length > 16) return '';
  return extension;
}

function attachmentBlock(attachment: Attachment): string {
  const size = attachment.size > 0 ? formatBytes(attachment.size) : 'unknown size';
  const heading = `### ${attachment.path} (${attachment.mode === 'inline' ? 'inlined' : 'uploaded'}, ${size})`;
  if (attachment.mode === 'inline') {
    const content = (attachment.content ?? '').replace(/\s+$/, '');
    const fence = fenceFor(content);
    const language = languageOf(attachment.path);
    return `${heading}\n\n${fence}${language}\n${content}\n${fence}`;
  }
  return `${heading}\n\nRead this file from the workspace when you need it: \`${attachment.path}\``;
}

/**
 * The message the server actually receives: the draft, then one block per ready
 * attachment. Returns the draft untouched when nothing is attached.
 */
export function composeMessage(draft: string, attachments: readonly Attachment[]): string {
  const ready = attachments.filter((attachment) => attachment.status === 'ready');
  if (ready.length === 0) return draft;
  const body = draft.replace(/\s+$/, '');
  const blocks = ready.map(attachmentBlock).join('\n\n');
  const header = `Attached ${ready.length === 1 ? 'file' : 'files'}:`;
  return body === '' ? `${header}\n\n${blocks}` : `${body}\n\n${header}\n\n${blocks}`;
}

/**
 * The chips a queued message carries, for a recall to put back.
 *
 * Uploads send only where the file landed; an inline file sends its text too,
 * because the browser's copy is the only one that exists - the server never saw
 * the file, only the block the composer rendered from it.
 */
export function attachForQueue(attachments: readonly Attachment[]): QueuedAttachment[] {
  return attachments
    .filter((attachment) => attachment.status === 'ready')
    .map((attachment) => ({
      // The name the user picked; `path` is where it went.
      name: attachment.name,
      path: attachment.path,
      mode: attachment.mode,
      size: attachment.size,
      ...(attachment.mode === 'inline' && attachment.content !== undefined
        ? { content: attachment.content }
        : {}),
    }));
}

/**
 * Rebuild composer chips from a recalled message.
 *
 * Ready by construction: the queued entry only carries attachments that were
 * ready when it was sent, and their content came back with them.
 */
export function attachmentsFromQueue(queued: readonly QueuedAttachment[]): Attachment[] {
  return queued.map((attachment, index) => ({
    id: `${newAttachmentId(attachment.name)}:${index}`,
    name: attachment.name,
    size: attachment.size,
    mode: attachment.mode,
    path: attachment.path,
    status: 'ready',
    ...(attachment.content !== undefined ? { content: attachment.content } : {}),
  }));
}