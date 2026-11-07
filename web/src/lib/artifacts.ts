// Chat artifacts: fenced blocks the transcript renders as live content instead of
// (or as well as) code.
//
// @kc-doc: artifacts
// @kc-title: Render rich artifacts in the chat transcript
// @kc-summary: An agent can put a diagram, an HTML mock, a table or a document in its reply, and the web UI draws it as a card with a live Preview and the Source it came from.
// @kc-tags: artifacts, web-ui, html, mermaid, diagram, svg, mock, preview, chat
// @kc-applies-to: route:POST /api/artifacts, route:GET /api/artifacts/:id, fence:artifact, fence:html, fence:mermaid, file:web/src/lib/artifacts.ts, file:web/src/components/ArtifactCard.tsx, concept:chat artifact, concept:ArtifactCard
//
// # What an artifact is
//
// A normal reply is Markdown. A **fenced block** in that reply whose language is
// renderable is drawn as a card instead of a code block: the transcript shows the
// thing itself - a mock you can scroll, a diagram, a table - with a **Preview /
// Source** switch, copy, download and a full-screen view. You do not call a tool
// to make one; you write a fence.
//
// # How to write one
//
// Name the fence `artifact`, then the language, then whatever metadata you want:
//
// ````text
// ```artifact html title="Signup form" height=420
// <!doctype html>
// <html><body><button>Sign up</button></body></html>
// ```
// ````
//
// The same thing with the metadata in a header block, if you prefer:
//
// ````text
// ```artifact
// title: Signup form
// type: html
// height: 420
// ---
// <button>Sign up</button>
// ```
// ````
//
// The header is only read when it is closed by a line containing just `---`;
// without it the lines are part of the artifact. `title`, `type` (alias `lang`,
// `language`, `kind`) and `height` are the metadata keys, accepted in the info
// string (`key=value`, quoted when it has spaces) and in the header block, with
// the info string winning.
//
// A fence that does not say `artifact` is still a card when its language is one
// of the renderable ones **and** its body is substantial - a real mock, a diagram,
// a table - while a three-line example stays a code block, so the transcript does
// not turn every snippet into a frame. Write `artifact` when you want the preview
// for certain.
//
// # What renders
//
// | Fence | Preview |
// | --- | --- |
// | `artifact html` / `html` | the markup, as a real page in an isolated frame (its own scripts run) |
// | `artifact svg` / `svg` | the drawing, inline, sanitised |
// | `artifact mermaid` / `mermaid` | the diagram, drawn by Mermaid |
// | `artifact markdown` / `markdown` | the document, rendered like the chat does |
// | `artifact json` / `json` | the payload, pretty-printed |
// | `artifact csv` / `tsv` | the table, header row and all |
// | `artifact image` | a `data:image/…` URL or workspace path fetched with API auth and rendered through a temporary blob URL |
//
// Without a declared type, the language of the fence decides, and an `artifact`
// fence with no language is sniffed: a tag means HTML, `graph TD` means Mermaid,
// `{` means JSON, matching delimiters mean a table.
//
// # What does not render
//
// * **A fence still being written.** Cards appear when the message finishes: a
//   half-written mock would otherwise paint, reload and relayout the transcript on
//   every token.
// * **Remote assets.** An artifact document is offline on purpose: inline script
//   and style run, but a CDN stylesheet, a web font or a remote image will not
//   load. Inline them (`data:`) or draw with CSS/SVG.
// * **Script inside an SVG.** An SVG artifact is drawn inline, in this app's
//   origin, so scripts and event handlers are stripped from it. Use an `html`
//   artifact when you need behaviour - that one runs in its own frame.
// * **Anything above 1 MiB.** That is the documented `413`; a mock is markup, not
//   an asset bundle.
//
// # The routes behind it
//
// The web client publishes the source and points an `<iframe>` at the answer:
//
// ```bash
// curl -s -X POST localhost:8787/api/artifacts \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"html":"<button>Buy</button>","theme":"dark"}'
// # → 201 {"id":"9f2c…","url":"/api/artifacts/9f2c…?capability=<random>","bytes":374,"theme":"dark"}
// ```
//
// `GET /api/artifacts/<id>?capability=<random>` serves that document with its own
// policy: an **opaque origin** (`sandbox allow-scripts`, never `allow-same-origin`)
// and `frame-ancestors 'self'` so only this app may embed it. Inline script/style
// run, but fetch/XHR/WebSocket, remote subresources and forms are blocked. A
// directly opened top-level artifact can still navigate itself: Chromium verifies
// this residual can disclose the viewing event and this one artifact's capability,
// never the master bearer, and `no-referrer` keeps the capability out of Referer
// headers. Everything is in memory, keyed by a SHA-256 of the bytes, and dropped
// when the process exits: posting the same artifact again is the same id.
// **Render again** (or **Try again** after a failure) explicitly invalidates the
// browser-cached capability and POSTs the source again, so an after-restart or
// after-eviction retry never reuses a dead URL. The authenticated POST returns the
// random, scoped capability; GET never accepts the deployment's master `?token=`.
//
// Full reference: `docs/API.md` (Chat artifacts) and `web/README.md` (how the
// renderer is put together).
//
// # How it fits together
//
// This module is pure: no DOM, no network, no Preact. `lib/markdown.ts` is the
// only caller that wires it into the renderer (a recognised fence becomes a
// placeholder element), and `components/ArtifactCard.tsx` decides how each kind
// is drawn.
//
// Nothing here decides how content is *drawn*, which is the safety property:
// HTML goes to an isolated document served by the server, SVG is sanitised and
// drawn in a shadow root, everything else is a first-party component over parsed
// data. A fenced block is data, never markup.

/** What a fence's content is, as far as the transcript can tell. */
export type ArtifactKind =
  | 'html'
  | 'svg'
  | 'mermaid'
  | 'markdown'
  | 'json'
  | 'csv'
  | 'tsv'
  | 'image'
  | 'unknown';

/** Which half of the card is showing. */
export type ArtifactView = 'preview' | 'source';

export interface ArtifactSpec {
  kind: ArtifactKind;
  /** Language to highlight the source with (`xml` for both HTML and SVG). */
  language: string;
  /** Short chrome label, e.g. `HTML`, `Mermaid`, `CSV`. */
  label: string;
  /** Author-supplied title, or `''`. */
  title: string;
  /** The artifact body, with any metadata header removed. */
  source: string;
  /** Preferred preview height in CSS pixels, already clamped; `null` = kind default. */
  height: number | null;
  /** The fence said `artifact`, so it is content first and code second. */
  explicit: boolean;
  /** Which half the card opens on. */
  initial: ArtifactView;
}

/** Languages whose fences are content, mapped to the kind they are. */
const KIND_BY_LANGUAGE: Record<string, ArtifactKind> = {
  html: 'html',
  htm: 'html',
  xhtml: 'html',
  html5: 'html',
  svg: 'svg',
  mermaid: 'mermaid',
  mmd: 'mermaid',
  markdown: 'markdown',
  md: 'markdown',
  json: 'json',
  jsonc: 'json',
  json5: 'json',
  csv: 'csv',
  tsv: 'tsv',
  image: 'image',
  img: 'image',
};

/** Highlight.js language per kind; `xml` covers HTML and SVG. */
const HIGHLIGHT_BY_KIND: Record<ArtifactKind, string> = {
  html: 'xml',
  svg: 'xml',
  mermaid: 'markdown',
  markdown: 'markdown',
  json: 'json',
  csv: 'markdown',
  tsv: 'markdown',
  image: 'markdown',
  unknown: 'markdown',
};

const LABEL_BY_KIND: Record<ArtifactKind, string> = {
  html: 'HTML',
  svg: 'SVG',
  mermaid: 'Mermaid',
  markdown: 'Markdown',
  json: 'JSON',
  csv: 'CSV',
  tsv: 'TSV',
  image: 'Image',
  unknown: 'Artifact',
};

/** Kinds whose *content* is the point, so an implicit fence opens on the preview. */
const PREVIEW_FIRST: ReadonlySet<ArtifactKind> = new Set<ArtifactKind>([
  'html',
  'svg',
  'mermaid',
  'csv',
  'tsv',
  'image',
]);

/**
 * Below this, an implicit fence is left alone. The number is deliberately not
 * generous: an example inside an explanation is usually a handful of lines, and
 * a card for it would be chrome around two lines of code.
 */
const SUBSTANTIAL_CHARS = 200;

/** A `---`-delimited header is only believed within this many lines of the top. */
const MAX_HEADER_LINES = 12;

/** Preview heights accepted from a fence; anything else is clamped. */
export const MIN_HEIGHT = 120;
export const MAX_HEIGHT = 1200;

/** The umbrella fence names. `artifact` is documented; `preview` is an alias. */
const UMBRELLA = new Set(['artifact', 'preview']);

/** Metadata keys accepted in the info string and the header block. */
const META_KEYS = new Set(['title', 'type', 'lang', 'language', 'kind', 'height']);

/**
 * Split an info string into an umbrella flag, a bare language and metadata.
 *
 * Accepts `artifact html title="Signup form" height=420` as well as
 * `html title='Signup form',height=420`. A quoted value may contain spaces, an
 * unquoted one ends at whitespace or a comma; that is enough for the metadata
 * this card has, and it means a model does not have to get JSON quoting right to
 * label a block.
 */
export function parseInfoString(info: string): {
  umbrella: boolean;
  language: string;
  meta: Record<string, string>;
} {
  const meta: Record<string, string> = {};
  const bare: string[] = [];
  let umbrella = false;
  for (const token of tokenizeInfo(info)) {
    const eq = token.indexOf('=');
    if (eq > 0) {
      const key = token.slice(0, eq).trim().toLowerCase();
      const value = unquote(token.slice(eq + 1).trim());
      if (META_KEYS.has(key)) meta[key] = value;
      continue;
    }
    const word = token.toLowerCase();
    if (UMBRELLA.has(word)) umbrella = true;
    else if (word !== '') bare.push(word);
  }
  return { umbrella, language: bare[0] ?? '', meta };
}

/** Whitespace- and comma-separated, but a quoted value keeps its spaces. */
function tokenizeInfo(info: string): string[] {
  const tokens: string[] = [];
  let current = '';
  let quote: '"' | "'" | null = null;
  for (const char of info) {
    if (quote) {
      current += char;
      if (char === quote) quote = null;
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      current += char;
      continue;
    }
    if (char === ',' || /\s/.test(char)) {
      if (current !== '') tokens.push(current);
      current = '';
      continue;
    }
    current += char;
  }
  if (current !== '') tokens.push(current);
  return tokens;
}

function unquote(value: string): string {
  if (value.length >= 2) {
    const first = value[0];
    if ((first === '"' || first === "'") && value.endsWith(first)) {
      return value.slice(1, -1);
    }
  }
  return value;
}

/**
 * Read a `key: value` header closed by a line containing only `---`.
 *
 * Strict on purpose. The terminator is what makes this unambiguous: without it a
 * Markdown artifact that happens to open with `Note: something` would lose its
 * first line to the metadata parser. When the header is not exactly right the
 * fence is rendered whole, metadata lines and all, which is a visible mistake
 * rather than a silent one.
 */
export function parseHeaderBlock(content: string): {
  meta: Record<string, string>;
  body: string;
} {
  const lines = content.split('\n');
  const limit = Math.min(lines.length, MAX_HEADER_LINES);
  const meta: Record<string, string> = {};
  for (let index = 0; index < limit; index += 1) {
    const line = lines[index] ?? '';
    const trimmed = line.trim();
    if (trimmed === '---') {
      if (index === 0) return { meta: {}, body: content };
      return { meta, body: lines.slice(index + 1).join('\n') };
    }
    const match = /^([A-Za-z][A-Za-z-]*)\s*:\s*(.+)$/.exec(trimmed);
    if (!match) return { meta: {}, body: content };
    const key = (match[1] ?? '').toLowerCase();
    // An unknown key means this is not our header: `Note: …` followed by a rule
    // is prose with a horizontal rule, and stripping its first line would be a
    // silent edit to the author's document.
    if (!META_KEYS.has(key)) return { meta: {}, body: content };
    meta[key] = unquote((match[2] ?? '').trim());
  }
  return { meta: {}, body: content };
}

/** A positive, clamped pixel height, or `null` when the fence did not ask for one. */
export function parseHeight(raw: string | undefined): number | null {
  if (raw === undefined) return null;
  const parsed = Number.parseInt(raw, 10);
  if (!Number.isFinite(parsed)) return null;
  return Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, parsed));
}

/**
 * Guess what a body is when the fence did not say.
 *
 * Ordered from most to least specific. Anything that starts with a tag is HTML —
 * a fragment is the common case for a mock and the frame renders it fine — which
 * is also the fallback, because "unknown text" in an artifact fence is far more
 * likely to be markup than to be something the UI should refuse to draw.
 */
export function sniffKind(source: string): ArtifactKind {
  const text = source.trim();
  const head = text.slice(0, 400).toLowerCase();
  if (text === '') return 'unknown';
  if (head.startsWith('data:image/')) return 'image';
  if (/^<svg[\s>]/.test(head)) return 'svg';
  if (head.includes('<!doctype html') || head.includes('<html')) return 'html';
  if (/^<(?:!doctype|div|section|main|header|footer|p|ul|ol|table|style|script|\?xml)/.test(head)) {
    return 'html';
  }
  // Mermaid's keywords are case-sensitive (`sequenceDiagram`, `gitGraph`), so
  // this one check reads the source rather than the lowercased copy.
  if (looksLikeMermaid(text.slice(0, 40))) return 'mermaid';
  if (/^[[{]/.test(text) && isJson(text)) return 'json';
  if (looksLikeDelimited(text)) return 'csv';
  if (/^#{1,6} \S/m.test(text) || /^\s*[-*] \S/m.test(text) || /^\s*\d+\. \S/m.test(text)) {
    return 'markdown';
  }
  return 'html';
}

/** Mermaid's own first tokens; a diagram type is always the first word. */
function looksLikeMermaid(head: string): boolean {
  return /^(?:graph|flowchart|sequenceDiagram|classDiagram|stateDiagram(?:-v2)?|erDiagram|gantt|pie|journey|gitGraph|mindmap|timeline|quadrantChart|xychart(?:-beta)?|sankey(?:-beta)?|block-beta|packet-beta|architecture-beta|requirementDiagram|c4Context|zenuml)\b/.test(
    head,
  );
}

function isJson(text: string): boolean {
  try {
    JSON.parse(text);
    return true;
  } catch {
    return false;
  }
}

/** Two or more lines that parse to the same number of columns. */
function looksLikeDelimited(text: string): boolean {
  const lines = text.split('\n').filter((line) => line.trim() !== '');
  if (lines.length < 2) return false;
  const sample = lines.slice(0, 5).join('\n');
  for (const delimiter of [',', '\t', ';', '|']) {
    const rows = parseTable(sample, delimiter);
    if (rows.length < 2) continue;
    const width = rows[0]?.length ?? 0;
    // A quoted field can carry the delimiter, so the count comes from the
    // parser rather than from counting characters.
    if (width > 1 && rows.every((row) => row.length === width)) return true;
  }
  return false;
}

/** Is this fence worth a card at all when the author did not say `artifact`? */
function isSubstantial(kind: ArtifactKind, source: string): boolean {
  const text = source.trim();
  switch (kind) {
    case 'mermaid':
      return true;
    case 'svg':
      return /^<svg[\s>]/i.test(text);
    case 'csv':
    case 'tsv':
      return looksLikeDelimited(text);
    case 'image':
      return false;
    case 'markdown':
      return text.length >= SUBSTANTIAL_CHARS && /^#{1,6} \S/m.test(text);
    case 'json':
      return text.length >= SUBSTANTIAL_CHARS && isJson(text);
    case 'html':
      return text.length >= SUBSTANTIAL_CHARS || /<!doctype|<html|<style|<script/i.test(text);
    case 'unknown':
      return false;
  }
}

/**
 * Turn one fenced block into an artifact, or return `null` for "render this as
 * code, exactly as before".
 *
 * `info` is the fence's info string (` ```html title="x" ` → `html title="x"`)
 * and `content` is everything between the fences, without them.
 */
export function parseFence(info: string, content: string): ArtifactSpec | null {
  const { umbrella, language, meta: infoMeta } = parseInfoString(info);
  const fencedKind: ArtifactKind | null = KIND_BY_LANGUAGE[language] ?? null;

  if (!umbrella && fencedKind === null) return null;

  let meta: Record<string, string> = infoMeta;
  let source = content;
  if (umbrella) {
    const header = parseHeaderBlock(content);
    if (header.body !== content) {
      // Info-string metadata wins over the header block: it is the more explicit
      // of the two, and it is the form the documented example uses.
      meta = { ...header.meta, ...infoMeta };
      source = header.body;
    }
  }

  const declared = (
    meta['type'] ??
    meta['lang'] ??
    meta['language'] ??
    meta['kind'] ??
    ''
  ).toLowerCase();
  const kind = resolveKind(declared, fencedKind, source);

  if (!umbrella && !isSubstantial(kind, source)) return null;

  const title = (meta['title'] ?? '').trim();
  const initial: ArtifactView =
    umbrella || PREVIEW_FIRST.has(kind) ? 'preview' : 'source';

  return {
    kind,
    language: HIGHLIGHT_BY_KIND[kind],
    label: LABEL_BY_KIND[kind],
    title,
    source,
    height: parseHeight(meta['height']),
    explicit: umbrella,
    initial,
  };
}

function resolveKind(
  declared: string,
  fencedKind: ArtifactKind | null,
  source: string,
): ArtifactKind {
  const fromDeclared = KIND_BY_LANGUAGE[declared];
  if (fromDeclared) return fromDeclared;
  if (fencedKind) return fencedKind;
  return sniffKind(source);
}

/** The text a screen reader announces for one card. */
export function describeArtifact(spec: ArtifactSpec): string {
  const name = spec.title !== '' ? `${spec.title} (${spec.label}) artifact` : `${spec.label} artifact`;
  return spec.kind === 'html' ? `${name}, rendered as an isolated page` : name;
}

/** One parsed row of a delimited table; values are already unquoted. */
export type TableRow = string[];

/**
 * Parse CSV/TSV/;-delimited text into rows.
 *
 * RFC 4180 quoting, because it is what real exports use: a quoted field may
 * contain the delimiter, a doubled quote (`""`) is one quote character, and a
 * quoted field may span several lines. CRLF is handled. Nothing here is
 * sanitised for HTML — the table component builds DOM nodes, not markup.
 */
export function parseTable(source: string, delimiter = ','): TableRow[] {
  const rows: TableRow[] = [];
  let row: string[] = [];
  let field = '';
  let quoted = false;

  for (let index = 0; index < source.length; index += 1) {
    const char = source[index] ?? '';
    if (quoted) {
      if (char === '"') {
        if (source[index + 1] === '"') {
          field += '"';
          index += 1;
        } else {
          quoted = false;
        }
      } else {
        field += char;
      }
      continue;
    }
    if (char === '"' && field === '') {
      quoted = true;
      continue;
    }
    if (char === delimiter) {
      row.push(field);
      field = '';
      continue;
    }
    if (char === '\n' || char === '\r') {
      if (char === '\r' && source[index + 1] === '\n') index += 1;
      row.push(field);
      rows.push(row);
      row = [];
      field = '';
      continue;
    }
    field += char;
  }
  if (field !== '' || row.length > 0) {
    row.push(field);
    rows.push(row);
  }
  return rows;
}

/** The delimiter a `csv`/`tsv` artifact actually uses. */
export function tableDelimiter(kind: ArtifactKind, source: string): string {
  if (kind === 'tsv') return '\t';
  const sample = source.slice(0, 2000);
  for (const delimiter of [',', '\t', ';', '|']) {
    if (sample.includes(delimiter)) return delimiter;
  }
  return ',';
}

/** Rows and columns are both capped: a 4000-column export must not be a page. */
export const MAX_TABLE_ROWS = 200;
export const MAX_TABLE_COLUMNS = 16;
/** Above this the table view is refused and the source is shown instead. */
export const MAX_TABLE_CHARS = 256_000;

export interface ArtifactTable {
  header: string[];
  rows: TableRow[];
  /** Rows that exist but are not shown, because of [`MAX_TABLE_ROWS`]. */
  hiddenRows: number;
  /** Columns that exist but are not shown, because of [`MAX_TABLE_COLUMNS`]. */
  hiddenColumns: number;
}

/**
 * Build the table a preview shows: the first row is the header, and everything
 * past the caps is counted rather than dropped silently.
 */
export function buildTable(spec: ArtifactSpec, delimiter?: string): ArtifactTable {
  const rows = parseTable(spec.source, delimiter ?? tableDelimiter(spec.kind, spec.source));
  const [first = [], ...rest] = rows;
  let columns = first.length;
  for (const row of rest) columns = Math.max(columns, row.length);
  return {
    header: first.slice(0, MAX_TABLE_COLUMNS),
    rows: rest.slice(0, MAX_TABLE_ROWS).map((row) => row.slice(0, MAX_TABLE_COLUMNS)),
    hiddenRows: Math.max(0, rest.length - MAX_TABLE_ROWS),
    hiddenColumns: Math.max(0, columns - MAX_TABLE_COLUMNS),
  };
}
