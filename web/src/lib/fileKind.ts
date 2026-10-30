/**
 * What the file viewer can do with a workspace file, decided from its name and
 * what the server said about it.
 *
 * The viewer has three shapes: highlighted source (everything text), a rendered
 * preview (Markdown, HTML, images, PDF) and a refusal (binary). Editing is only
 * ever offered for text that round-trips through the save endpoint, so this
 * module is the single place that answers "can I render this" and "can I edit
 * this" — the two answers are deliberately different.
 */

/** How a file is shown. `code` and `preview` are both editable when text. */
export type FileRender = 'code' | 'markdown' | 'html' | 'image' | 'pdf' | 'binary';

export interface FileFacts {
  /** Workspace-relative path, e.g. `docs/guide.md`. */
  path: string;
  /** `language` as reported by `GET /api/workspace/file` (may be null). */
  language?: string | null;
  /** The server found a NUL byte / invalid UTF-8 in it. */
  binary?: boolean;
  /** The server stopped at `limits.max_file_read_bytes`. */
  truncated?: boolean;
}

export interface FileCapability {
  render: FileRender;
  /** The viewer can offer the editor: text that is complete and round-trips. */
  canEdit: boolean;
  /** The editor starts in the rendered view rather than the source view. */
  preferPreview: boolean;
  /** Shown instead of the editor when a file cannot be edited, and why. */
  editBlockedReason: string | null;
}

const IMAGE_EXTENSIONS = new Set([
  'png',
  'jpg',
  'jpeg',
  'gif',
  'webp',
  'avif',
  'bmp',
  'ico',
  'svg',
]);

const MARKDOWN_EXTENSIONS = new Set(['md', 'markdown', 'mdx']);
const HTML_EXTENSIONS = new Set(['html', 'htm', 'xhtml']);
const PDF_EXTENSIONS = new Set(['pdf']);

/** Lowercased extension of the last path segment, or `''` when there is none. */
export function fileExtension(path: string): string {
  const name = path.slice(path.lastIndexOf('/') + 1);
  const dot = name.lastIndexOf('.');
  // A dotfile (`.env`, `.gitignore`) has no extension: the whole name is the stem.
  return dot <= 0 ? '' : name.slice(dot + 1).toLowerCase();
}

export function fileName(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1);
}

/**
 * Decide how to present one file.
 *
 * `binary` from the server wins over the extension: a `.svg`-named file full of
 * NUL bytes is still binary. The reverse is not true — a PDF is valid text with
 * no NUL byte anywhere (`%PDF-1.4` is ASCII), so the server can call it "text" —
 * which is why an extension that is an image or a PDF is treated as one either
 * way. Truncated text can be shown but never edited: a save would silently drop
 * everything past the read limit.
 */
export function describeFile(file: FileFacts): FileCapability {
  const extension = fileExtension(file.path);
  const language = (file.language ?? '').toLowerCase();

  // Images and PDFs are served from the raw-bytes endpoint. The extension decides,
  // not the server's NUL-byte sniff: a PNG is binary, but a PDF and an SVG are
  // text, so the sniffer calls them "text" while the browser must still be handed
  // the bytes rather than a highlighted dump of `%PDF-1.4` or `<svg`.
  if (IMAGE_EXTENSIONS.has(extension)) {
    return {
      render: 'image',
      canEdit: false,
      preferPreview: true,
      editBlockedReason: 'Images are shown as-is; there is no text to edit.',
    };
  }
  if (PDF_EXTENSIONS.has(extension)) {
    return {
      render: 'pdf',
      canEdit: false,
      preferPreview: true,
      editBlockedReason: 'PDFs are shown as-is; there is no text to edit.',
    };
  }

  if (file.binary) {
    return {
      render: 'binary',
      canEdit: false,
      preferPreview: false,
      editBlockedReason: 'This is a binary file, so there is no text to edit.',
    };
  }

  const truncatedReason = file.truncated
    ? 'The server truncated this file at limits.max_file_read_bytes, so saving it would discard the rest. Raise the limit, or edit it with the agent.'
    : null;

  if (MARKDOWN_EXTENSIONS.has(extension) || language === 'markdown') {
    return {
      render: 'markdown',
      canEdit: truncatedReason === null,
      preferPreview: true,
      editBlockedReason: truncatedReason,
    };
  }
  if (HTML_EXTENSIONS.has(extension) || language === 'html') {
    return {
      render: 'html',
      canEdit: truncatedReason === null,
      preferPreview: true,
      editBlockedReason: truncatedReason,
    };
  }
  return {
    render: 'code',
    canEdit: truncatedReason === null,
    preferPreview: false,
    editBlockedReason: truncatedReason,
  };
}

/** `true` when the rendered preview has something to show (not the source view). */
export function hasRichPreview(render: FileRender): boolean {
  return render !== 'code' && render !== 'binary';
}

/**
 * `true` when the file has text the source view can show, which is also what the
 * Preview/Source toggle needs to exist at all. An image or a PDF has no source
 * view — and note that `binary: false` does not imply text, because a PDF is ASCII.
 */
export function hasSourceView(render: FileRender): boolean {
  return render === 'code' || render === 'markdown' || render === 'html';
}

/**
 * `true` for a reference that is not a workspace file at all: an absolute URL
 * (`https:`, `mailto:`, `data:`), a protocol-relative URL, or a bare fragment.
 * Those are left exactly as the author wrote them.
 */
export function isExternalReference(target: string): boolean {
  const cleaned = target.trim();
  if (cleaned === '') return true;
  if (cleaned.startsWith('#')) return true;
  if (cleaned.startsWith('//')) return true;
  return /^[a-z][a-z0-9+.-]*:/i.test(cleaned);
}

/**
 * Path relative to the file's own directory, for resolving a relative link or
 * image reference inside a Markdown or HTML preview.
 *
 * `docs/guide.md` + `../assets/logo.png` → `assets/logo.png`; a leading `/` is
 * already workspace-relative and is returned without the slash. Returns `null`
 * for anything that climbs above the workspace root or is an absolute URL.
 */
export function resolveRelativePath(fromFile: string, target: string): string | null {
  const cleaned = target.trim();
  if (isExternalReference(cleaned)) return null;

  const withoutQuery = cleaned.split(/[?#]/, 1)[0] ?? cleaned;
  if (withoutQuery === '') return null;

  const segments = cleaned.startsWith('/')
    ? []
    : fromFile.slice(0, fromFile.lastIndexOf('/') + 1).split('/').filter(Boolean);
  const parts = withoutQuery.split('/');
  for (const part of parts) {
    if (part === '' || part === '.') continue;
    if (part === '..') {
      if (segments.length === 0) return null;
      segments.pop();
      continue;
    }
    let decoded = part;
    try {
      decoded = decodeURIComponent(part);
    } catch {
      // A malformed escape is taken literally rather than dropped.
    }
    if (decoded.includes('\0')) return null;
    segments.push(decoded);
  }
  return segments.length === 0 ? null : segments.join('/');
}
