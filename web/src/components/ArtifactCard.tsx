import type { JSX } from 'preact';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import {
  MAX_TABLE_CHARS,
  buildTable,
  describeArtifact,
  type ArtifactKind,
  type ArtifactSpec,
  type ArtifactView,
} from '../lib/artifacts';
import { invalidateArtifact, publishArtifact } from '../lib/artifactPreview';
import { copyText } from '../lib/clipboard';
import { downloadText, safeFilename } from '../lib/download';
import { renderCodeBlock, renderMarkdown } from '../lib/markdown';
import { renderMermaid } from '../lib/mermaid';
import { hasSvgElement, sanitizeSvg } from '../lib/svg';
import * as store from '../state/store';
import { activeTheme } from '../theme';
import { Dialog } from './Dialog';
import { useWorkspaceObjectUrl } from './FilePreview';
import {
  AlertIcon,
  ArtifactIcon,
  CopyIcon,
  DownloadIcon,
  ExpandIcon,
  RefreshIcon,
} from './icons';

/**
 * The default preview height of a page. A frame has no intrinsic height, so a
 * mock with no `height=` would otherwise collapse to nothing; this is the height
 * it gets until (or unless) the document reports its own.
 */
const DEFAULT_HTML_HEIGHT = 420;

/** A measured height is clamped to this range: a mock sizes its card, not the page. */
const MIN_FRAMED_HEIGHT = 80;
const MAX_FRAMED_HEIGHT = 720;

const DOWNLOAD_EXTENSION: Record<ArtifactKind, string> = {
  html: 'html',
  svg: 'svg',
  mermaid: 'mmd',
  markdown: 'md',
  json: 'json',
  csv: 'csv',
  tsv: 'tsv',
  image: 'txt',
  unknown: 'txt',
};

const DOWNLOAD_MIME: Record<string, string> = {
  html: 'text/html',
  svg: 'image/svg+xml',
  json: 'application/json',
  csv: 'text/csv',
  tsv: 'text/tab-separated-values',
  md: 'text/markdown',
};

/** The one-line explanation each preview kind needs, when it needs one. */
function previewNote(kind: ArtifactKind): string | null {
  if (kind === 'html') {
    return 'Rendered in an isolated frame: its own scripts run, but it has no network access and cannot reach karucode.';
  }
  return null;
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'The preview could not be rendered.';
}

type AsyncState =
  | { status: 'loading' }
  | { status: 'ready'; html: string; url: string }
  | { status: 'error'; message: string };

/** A short "Rendering…" line, announced once, so a slow preview is not silent. */
function Loading(): JSX.Element {
  return (
    <p class="artifact-loading" role="status">
      Rendering…
    </p>
  );
}

/** What went wrong, why it might be the content's fault, and a way out. */
function Failure({
  message,
  onRetry,
  onShowSource,
}: {
  message: string;
  onRetry?: () => void;
  onShowSource: () => void;
}): JSX.Element {
  return (
    <div class="artifact-failure" role="alert" data-testid="artifact-error">
      <p class="artifact-failure-head">
        <AlertIcon size={13} />
        <span>This artifact could not be rendered</span>
      </p>
      <p class="artifact-failure-message">{message}</p>
      <div class="artifact-failure-actions">
        <button type="button" class="artifact-button" onClick={onShowSource}>
          Show source
        </button>
        {onRetry ? (
          <button type="button" class="artifact-button" onClick={onRetry}>
            Try again
          </button>
        ) : null}
      </div>
    </div>
  );
}

/**
 * An HTML artifact: a real document in a real frame.
 *
 * The frame's `src` is an `/api/artifacts/<id>` URL, not `srcdoc`. A `srcdoc`
 * frame inherits this app's CSP (`default-src 'self'`), then runs in an opaque
 * origin where `'self'` matches nothing, so its inline `<style>` and `<script>`
 * are both refused and the mock renders nearly blank; it also has no base URL, so
 * nothing relative resolves. The server route gives the document its own policy
 * (opaque origin, inline script and style yes, network no).
 *
 * `sandbox="allow-scripts"` repeats the server's own `sandbox` directive on the
 * element. That is deliberate belt and braces: it never adds `allow-same-origin`,
 * so even if the route's header were lost the frame could not see this origin.
 */
function HtmlPreview({
  spec,
  theme,
  nonce,
  height,
  onShowSource,
}: {
  spec: ArtifactSpec;
  theme: 'light' | 'dark';
  nonce: number;
  height: number;
  onShowSource: () => void;
}): JSX.Element {
  const [state, setState] = useState<AsyncState>({ status: 'loading' });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setState({ status: 'loading' });
    publishArtifact(spec.source, theme)
      .then((base) => {
        if (cancelled) return;
        // `publishArtifact` returns the server-issued per-artifact capability
        // URL. Never replace it with a URL bearing the app's master credential.
        // The reload nonce is a separate query key, so the capability is intact.
        const url = nonce > 0 ? `${base}${base.includes('?') ? '&' : '?'}v=${nonce}` : base;
        setState({ status: 'ready', html: '', url });
      })
      .catch((error: unknown) => {
        if (!cancelled) setState({ status: 'error', message: errorMessage(error) });
      });
    return () => {
      cancelled = true;
    };
  }, [spec.source, theme, nonce, attempt]);

  /**
   * The document measures itself and posts its height (see the server's resize
   * reporter). Only a message from *this* frame's window and of the documented
   * type is believed, and the height is clamped: a mock can make its own card
   * tall, and nothing else.
   */
  const frameRef = useRef<HTMLIFrameElement>(null);
  const [measured, setMeasured] = useState<number | null>(null);
  useEffect(() => {
    const onMessage = (event: MessageEvent): void => {
      const frame = frameRef.current;
      if (!frame || event.source !== frame.contentWindow) return;
      const data = event.data as { type?: unknown; height?: unknown } | null;
      if (!data || data.type !== 'karucode-artifact') return;
      if (typeof data.height !== 'number' || !Number.isFinite(data.height)) return;
      setMeasured(Math.round(Math.min(MAX_FRAMED_HEIGHT, Math.max(MIN_FRAMED_HEIGHT, data.height))));
    };
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, []);

  if (state.status === 'loading') return <Loading />;
  if (state.status === 'error') {
    return (
      <Failure
        message={state.message}
        onRetry={() => {
          invalidateArtifact(spec.source, theme);
          setAttempt((value) => value + 1);
        }}
        onShowSource={onShowSource}
      />
    );
  }
  return (
    <iframe
      class="artifact-frame"
      data-testid="artifact-frame"
      sandbox="allow-scripts"
      referrerPolicy="no-referrer"
      loading="lazy"
      src={state.url}
      title={describeArtifact(spec)}
      ref={frameRef}
      style={{ height: `${spec.height ?? measured ?? height}px` }}
    />
  );
}

/**
 * Sanitised SVG, drawn inside a shadow root.
 *
 * Inline SVG is the one artifact kind that shares this app's origin, and an SVG
 * can carry a `<style>` element whose selectors apply to the *whole document* -
 * a mock with `text { fill: red }` would restyle every icon in the transcript,
 * and `svg { display: none }` would empty it. A shadow root is what makes the
 * drawing's own CSS apply to the drawing and nothing else, in both directions.
 *
 * It is also why the drawing is not in the vnode tree: a shadow root is built
 * imperatively (`attachShadow` + `innerHTML`), which is exactly what the
 * sanitiser's output already is.
 */
function ShadowSvg({
  html,
  label,
  testId,
  className,
}: {
  html: string;
  label: string;
  testId: string;
  className: string;
}): JSX.Element {
  const hostRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host) return undefined;
    const root = host.shadowRoot ?? host.attachShadow({ mode: 'open' });
    // The frame's own baseline: the drawing is centred, never wider than the
    // card, and its intrinsic size decides the height.
    // Size caps arrive as a custom property because they inherit into a shadow
    // root and a light-DOM selector cannot reach this `svg`. Without the height cap
    // an SVG keeps its intrinsic size and the card scrolls a cropped diagram.
    root.innerHTML =
      '<style>:host{display:block}svg{display:block;max-width:100%;' +
      'max-height:var(--artifact-svg-max-h, 420px);height:auto;margin:0 auto}</style>' +
      html;
    return () => {
      root.innerHTML = '';
    };
  }, [html]);

  return (
    <div
      class={className}
      data-testid={testId}
      role="img"
      aria-label={label}
      ref={hostRef}
    />
  );
}

/** An SVG artifact, sanitised and drawn inline so it can use the page's theme. */
function SvgPreview({ spec }: { spec: ArtifactSpec }): JSX.Element {
  const { html, ok } = useMemo(() => {
    const sanitized = sanitizeSvg(spec.source);
    return { html: sanitized, ok: hasSvgElement(sanitized) };
  }, [spec.source]);

  if (!ok) {
    return (
      <p class="artifact-failure" role="alert" data-testid="artifact-error">
        This SVG has no drawable content after sanitising — one that is only a
        &lt;script&gt;, or an empty &lt;svg&gt;, renders nothing.
      </p>
    );
  }
  return (
    <ShadowSvg
      html={html}
      label={describeArtifact(spec)}
      testId="artifact-svg"
      className="artifact-svg"
    />
  );
}

/** A Mermaid diagram: loaded on demand, rendered to SVG, then sanitised again. */
function MermaidPreview({
  spec,
  onShowSource,
}: {
  spec: ArtifactSpec;
  onShowSource: () => void;
}): JSX.Element {
  const theme = activeTheme.value.mode === 'dark' ? 'dark' : 'light';
  const [state, setState] = useState<AsyncState>({ status: 'loading' });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setState({ status: 'loading' });
    renderMermaid(spec.source, theme)
      .then((svg) => {
        if (cancelled) return;
        const sanitized = sanitizeSvg(svg);
        if (!hasSvgElement(sanitized)) {
          setState({
            status: 'error',
            message: 'Mermaid produced no diagram for this definition.',
          });
          return;
        }
        setState({ status: 'ready', html: sanitized, url: '' });
      })
      .catch((error: unknown) => {
        if (!cancelled) setState({ status: 'error', message: errorMessage(error) });
      });
    return () => {
      cancelled = true;
    };
  }, [spec.source, theme, attempt]);

  if (state.status === 'loading') return <Loading />;
  if (state.status === 'error') {
    return (
      <Failure
        message={state.message}
        onRetry={() => setAttempt((value) => value + 1)}
        onShowSource={onShowSource}
      />
    );
  }
  return (
    <ShadowSvg
      html={state.html}
      label={describeArtifact(spec)}
      testId="artifact-mermaid"
      className="artifact-diagram"
    />
  );
}

/** Markdown rendered by the same sanitiser the chat itself uses. */
function MarkdownPreview({ spec }: { spec: ArtifactSpec }): JSX.Element {
  // Artifacts are *not* re-enabled inside an artifact: a Markdown document that
  // quotes a fence shows the fence.
  const html = useMemo(() => renderMarkdown(spec.source), [spec.source]);
  const markup = useMemo(() => ({ __html: html }), [html]);
  return (
    <div
      class="markdown artifact-markdown"
      data-testid="artifact-markdown"
      dangerouslySetInnerHTML={markup}
    />
  );
}

/** JSON, pretty-printed. Minified payloads are the reason this exists. */
function JsonPreview({ spec }: { spec: ArtifactSpec }): JSX.Element {
  const parsed = useMemo(() => {
    try {
      return { ok: true as const, text: JSON.stringify(JSON.parse(spec.source), null, 2) };
    } catch (error) {
      return { ok: false as const, text: errorMessage(error) };
    }
  }, [spec.source]);

  if (!parsed.ok) {
    return (
      <div class="artifact-failure" role="alert" data-testid="artifact-error">
        <p class="artifact-failure-head">
          <AlertIcon size={13} />
          <span>This is not valid JSON</span>
        </p>
        <p class="artifact-failure-message">{parsed.text}</p>
      </div>
    );
  }
  // No code-block head: the card's own header names the artifact and owns the
  // copy button, so a second one inside the body would be a duplicate.
  const html = renderCodeBlock(parsed.text, 'json', 'JSON', { chrome: false });
  return (
    <div
      class="artifact-json"
      data-testid="artifact-json"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

/** A number, as a table cell reads: digits, an optional sign and one decimal part. */
function isNumericCell(value: string): boolean {
  return /^-?\d+(?:\.\d+)?%?$/.test(value.trim());
}

/** A delimited table. Rows and columns are capped, and it says so when it caps. */
function TablePreview({ spec }: { spec: ArtifactSpec }): JSX.Element {
  const oversized = spec.source.length > MAX_TABLE_CHARS;
  const table = useMemo(() => (oversized ? null : buildTable(spec)), [spec, oversized]);

  /**
   * Which columns are all numbers, so they can be set right-aligned with tabular
   * figures. A quantity column that reads `4` / `1,204` is the common case for a
   * CSV artifact and left-aligned numbers are hard to compare.
   */
  const numericColumns = useMemo(() => {
    if (table === null) return [];
    return table.header.map((_, index) => {
      const values = table.rows.map((row) => (row[index] ?? '').trim()).filter((value) => value !== '');
      return values.length > 0 && values.every(isNumericCell);
    });
  }, [table]);

  if (oversized || table === null) {
    return (
      <p class="artifact-notice" data-testid="artifact-error">
        This table is larger than {Math.round(MAX_TABLE_CHARS / 1000)} KB, so it is shown as
        source only.
      </p>
    );
  }
  return (
    <div class="artifact-table-wrap">
      <table class="artifact-table" data-testid="artifact-table">
        <caption class="sr-only">{spec.title || 'Artifact table'}</caption>
        <thead>
          <tr>
            {table.header.map((cell, index) => (
              <th scope="col" key={`h-${index}`} class={numericColumns[index] ? 'is-numeric' : undefined}>
                {cell}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {table.rows.map((row, rowIndex) => (
            <tr key={`r-${rowIndex}`}>
              {row.map((cell, columnIndex) => (
                <td
                  key={`c-${rowIndex}-${columnIndex}`}
                  class={numericColumns[columnIndex] ? 'is-numeric' : undefined}
                >
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {table.hiddenRows > 0 || table.hiddenColumns > 0 ? (
        <p class="artifact-caption">
          {table.hiddenRows > 0 ? `${table.hiddenRows} more rows` : ''}
          {table.hiddenRows > 0 && table.hiddenColumns > 0 ? ' and ' : ''}
          {table.hiddenColumns > 0 ? `${table.hiddenColumns} more columns` : ''}
          {' are not shown; the source has all of it.'}
        </p>
      ) : null}
    </div>
  );
}

/** An image artifact: a `data:` URL, or an authenticated workspace object URL. */
function ImagePreview({ spec }: { spec: ArtifactSpec }): JSX.Element {
  const source = spec.source.trim();
  const dataUrl = /^data:image\//i.test(source);
  const external = /^[a-z][a-z0-9+.-]*:/i.test(source) && !dataUrl;
  const project = store.currentProjectId.value;
  const media = useWorkspaceObjectUrl(!external && !dataUrl ? source : null, project);

  if (external) {
    return (
      <p class="artifact-notice" data-testid="artifact-error">
        An image artifact must be a <code>data:image/…</code> URL or a path inside the
        workspace; <code>{source.slice(0, 80)}</code> is neither.
      </p>
    );
  }
  if (media.error) {
    return (
      <p class="artifact-notice" role="alert" data-testid="artifact-error">
        Could not load {source}: {media.error}
      </p>
    );
  }
  if (!dataUrl && !media.url) {
    return <p class="artifact-notice" role="status">Loading artifact image…</p>;
  }
  return (
    <img
      class="artifact-image"
      data-testid="artifact-image"
      src={dataUrl ? source : (media.url ?? undefined)}
      alt={spec.title || 'Artifact image'}
      loading="lazy"
    />
  );
}

/**
 * The preview half: one renderer per kind, and a plain answer for the rest.
 *
 * `onShowSource` is handed down so every failure state has a way out: a diagram
 * that does not parse, a JSON payload that does not parse and a frame that could
 * not be published all offer the source, which is the artifact.
 */
function ArtifactPreview({
  spec,
  nonce,
  height,
  theme,
  onShowSource,
}: {
  spec: ArtifactSpec;
  nonce: number;
  height: number;
  theme: 'light' | 'dark';
  onShowSource: () => void;
}): JSX.Element {
  switch (spec.kind) {
    case 'html':
      return (
        <HtmlPreview
          spec={spec}
          theme={theme}
          nonce={nonce}
          height={height}
          onShowSource={onShowSource}
        />
      );
    case 'svg':
      return <SvgPreview spec={spec} />;
    case 'mermaid':
      return <MermaidPreview spec={spec} onShowSource={onShowSource} />;
    case 'markdown':
      return <MarkdownPreview spec={spec} />;
    case 'json':
      return <JsonPreview spec={spec} />;
    case 'csv':
    case 'tsv':
      return <TablePreview spec={spec} />;
    case 'image':
      return <ImagePreview spec={spec} />;
    default:
      return (
        <p class="artifact-notice">
          This block is not one of the renderable kinds. The source is below.
        </p>
      );
  }
}

/**
 * One artifact in the transcript: a fenced block that is shown as content.
 *
 * The card is chrome (a kind label, a title, Preview/Source, copy, download,
 * expand) around a preview or the source it came from. Three rules shape it:
 *
 * * **The source is always one click away.** Every failure path offers it, and
 *   every card can show it, because a preview is an interpretation of the text
 *   and the text is the artifact.
 * * **Nothing is rendered while the message streams.** `Markdown` turns cards off
 *   for a streaming message: markdown-it closes a fence at the end of its input,
 *   so a half-written mock would otherwise paint, reload and relayout on every
 *   token.
 * * **Each kind is drawn by an isolation level it deserves.** An HTML mock runs in
 *   another origin; an SVG is sanitised and drawn inline; a diagram becomes SVG
 *   through Mermaid and is sanitised again; everything else is first-party
 *   components over parsed data.
 */
export function ArtifactCard({ spec }: { spec: ArtifactSpec }): JSX.Element {
  const theme = activeTheme.value.mode === 'dark' ? 'dark' : 'light';
  const [view, setView] = useState<ArtifactView>(spec.initial);
  const [expanded, setExpanded] = useState(false);
  const [nonce, setNonce] = useState(0);
  const [copied, setCopied] = useState<'idle' | 'done' | 'failed'>('idle');
  const [announcement, setAnnouncement] = useState('');
  const copyTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
    },
    [],
  );

  const title = spec.title !== '' ? spec.title : spec.label;
  const height = spec.height ?? DEFAULT_HTML_HEIGHT;

  const sourcePane = useMemo(() => {
    const html = renderCodeBlock(spec.source, spec.language, spec.label, { chrome: false });
    return { __html: html };
  }, [spec.source, spec.language, spec.label]);

  const onCopy = (): void => {
    void copyText(spec.source).then((ok) => {
      setCopied(ok ? 'done' : 'failed');
      // Announced through the live region below, not just painted: the button's
      // `aria-label` is its name, so a rejected copy would otherwise change
      // nothing a screen reader could hear.
      setAnnouncement(
        ok
          ? 'Artifact source copied to the clipboard.'
          : 'Copy failed: the clipboard is unavailable here. Use Download instead.',
      );
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => {
        setCopied('idle');
        setAnnouncement('');
      }, 1600);
    });
  };

  const onDownload = (): void => {
    const extension = DOWNLOAD_EXTENSION[spec.kind];
    const name = safeFilename(spec.title !== '' ? spec.title : `artifact`, 'artifact');
    downloadText(`${name}.${extension}`, spec.source, DOWNLOAD_MIME[extension] ?? 'text/plain');
  };

  /** Every failure state's way out: show the text, and get out of full screen. */
  const showSource = (): void => {
    setView('source');
    setExpanded(false);
  };

  const body =
    view === 'source' ? (
      <div class="artifact-source" data-testid="artifact-source" dangerouslySetInnerHTML={sourcePane} />
    ) : (
      <div class="artifact-preview" data-testid="artifact-preview">
        <ArtifactPreview
          spec={spec}
          nonce={nonce}
          height={height}
          theme={theme}
          onShowSource={showSource}
        />
      </div>
    );

  const note = view === 'preview' ? previewNote(spec.kind) : null;

  return (
    <div
      class="artifact"
      data-kind={spec.kind}
      data-view={view}
      data-testid="artifact-card"
      role="group"
      aria-label={describeArtifact(spec)}
    >
      <div class="artifact-head">
        <span class="artifact-kind" aria-hidden="true">
          <ArtifactIcon size={12} />
          {spec.label}
        </span>
        {spec.title !== '' ? (
          <span class="artifact-title" title={spec.title}>
            {spec.title}
          </span>
        ) : null}
        <div class="artifact-views" role="group" aria-label="Artifact view">
          <button
            type="button"
            class="artifact-view"
            aria-pressed={view === 'preview'}
            onClick={() => setView('preview')}
          >
            Preview
          </button>
          <button
            type="button"
            class="artifact-view"
            aria-pressed={view === 'source'}
            onClick={() => setView('source')}
          >
            Source
          </button>
        </div>
        <div class="artifact-tools">
          {spec.kind === 'html' || spec.kind === 'mermaid' ? (
            <button
              type="button"
              class="artifact-tool"
              title="Render again"
              aria-label="Render this artifact again"
              onClick={() => {
                if (spec.kind === 'html') invalidateArtifact(spec.source, theme);
                setNonce((value) => value + 1);
              }}
            >
              <RefreshIcon size={13} />
            </button>
          ) : null}
          <button
            type="button"
            class="artifact-tool"
            data-copy-state={copied}
            title={
              copied === 'done'
                ? 'Copied'
                : copied === 'failed'
                  ? 'Copy failed — use Download'
                  : 'Copy source'
            }
            aria-label={
              copied === 'failed'
                ? "Copy failed — copy this artifact's source again"
                : "Copy this artifact's source"
            }
            onClick={onCopy}
          >
            {copied === 'done' ? (
              <span class="artifact-tool-text">Copied</span>
            ) : copied === 'failed' ? (
              <>
                <AlertIcon size={13} />
                <span class="artifact-tool-text">Copy failed</span>
              </>
            ) : (
              <CopyIcon size={13} />
            )}
          </button>
          <button
            type="button"
            class="artifact-tool"
            title="Download"
            aria-label={`Download this artifact as .${DOWNLOAD_EXTENSION[spec.kind]}`}
            onClick={onDownload}
          >
            <DownloadIcon size={13} />
          </button>
          <button
            type="button"
            class="artifact-tool"
            title="Expand"
            aria-label="Open this artifact full screen"
            onClick={() => setExpanded(true)}
          >
            <ExpandIcon size={13} />
          </button>
        </div>
      </div>
      <div class="artifact-body">{body}</div>
      {note !== null ? <p class="artifact-note">{note}</p> : null}
      {/* Copy result announced, not just painted: the tool button's name is its
          `aria-label`, so the visible "Copied"/"Copy failed" text alone is not a
          change assistive technology reports. `aria-live` rather than
          `role="status"` keeps the transcript's own status regions unambiguous. */}
      <span
        class="sr-only"
        aria-live="polite"
        aria-atomic="true"
        data-testid="artifact-announcement"
      >
        {announcement}
      </span>
      <Dialog
        open={expanded}
        title={title}
        description={describeArtifact(spec)}
        onClose={() => setExpanded(false)}
        class="dialog-artifact"
      >
        <div class="artifact-expanded">
          <ArtifactPreview
            spec={spec}
            nonce={nonce}
            height={height}
            theme={theme}
            onShowSource={showSource}
          />
        </div>
      </Dialog>
    </div>
  );
}
