import type { JSX } from 'preact';
import { useMemo } from 'preact/hooks';
import { workspaceRawUrl } from '../api/endpoints';
import { copyText } from '../lib/clipboard';
import { isExternalReference, resolveRelativePath, type FileRender } from '../lib/fileKind';
import { renderMarkdown } from '../lib/markdown';

export interface FilePreviewProps {
  /** Workspace-relative path of the file being previewed. */
  path: string;
  /** Current contents (the editor's live text when editing, else the loaded file). */
  content: string;
  render: FileRender;
  project?: string | null;
  /** The editor holds unsaved changes, so images/PDFs on disk are stale. */
  dirty?: boolean;
  /** Open a workspace path that a rendered link pointed at. */
  onOpenFile?: (path: string) => void;
}

/**
 * The rendered half of the file viewer.
 *
 * Everything that can render gets a preview here: Markdown through the same
 * renderer (and sanitiser) the chat uses, HTML inside a fully sandboxed iframe,
 * images and PDFs from the raw-bytes endpoint. Nothing from the workspace is ever
 * inserted into the app's own DOM as markup, and nothing can run: `renderMarkdown`
 * disables raw HTML, and the iframe has no `allow-scripts`, so a workspace HTML
 * file cannot reach the app's origin, storage or API.
 */
export function FilePreview({
  path,
  content,
  render,
  project = null,
  dirty = false,
  onOpenFile,
}: FilePreviewProps): JSX.Element {
  const markdownHtml = useMemo(
    () => (render === 'markdown' ? renderMarkdownForFile(content, path, project) : ''),
    [render, content, path, project],
  );
  const htmlSource = useMemo(
    () => (render === 'html' ? htmlDocument(content) : ''),
    [render, content],
  );

  if (render === 'markdown') {
    const onClick = (event: JSX.TargetedMouseEvent<HTMLDivElement>): void => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      if (handleCopyClick(target, content)) return;
      const anchor = target.closest<HTMLAnchorElement>('a[data-workspace-path]');
      const targetPath = anchor?.getAttribute('data-workspace-path');
      if (!anchor || !targetPath || !onOpenFile) return;
      event.preventDefault();
      onOpenFile(targetPath);
    };
    return (
      <div
        class="file-preview-scroll file-preview-markdown markdown"
        role="region"
        aria-label={`Rendered contents of ${path}`}
        onClick={onClick}
        dangerouslySetInnerHTML={{ __html: markdownHtml }}
        data-testid="file-preview-markdown"
      />
    );
  }

  if (render === 'image') {
    const url = workspaceRawUrl(path, project);
    return (
      <div class="file-preview-scroll file-preview-media" role="region" aria-label={`Image ${path}`}>
        {/* `alt` names the file: the viewer cannot know what it depicts. */}
        <img class="file-preview-image" src={url} alt={path} data-testid="file-preview-image" />
        {dirty ? (
          <p class="file-preview-note">
            The preview shows the saved file; there is no unsaved edit to show.
          </p>
        ) : null}
      </div>
    );
  }

  if (render === 'pdf') {
    const url = workspaceRawUrl(path, project);
    return (
      <div class="file-preview-scroll file-preview-media" role="region" aria-label={`PDF ${path}`}>
        <iframe class="file-preview-pdf" src={url} title={path} data-testid="file-preview-pdf" />
        <p class="file-preview-note">
          If the PDF does not render,{' '}
          <a href={url} target="_blank" rel="noopener noreferrer">
            open it in a new tab
          </a>
          .
        </p>
      </div>
    );
  }

  if (render === 'html') {
    return (
      <div
        class="file-preview-scroll file-preview-media"
        role="region"
        aria-label={`Rendered HTML of ${path}`}
      >
        <iframe
          class="file-preview-html"
          // No `allow-scripts`, `allow-same-origin` or `allow-forms`: the document
          // is inert, so a workspace file cannot script the app or submit anything.
          sandbox=""
          referrerPolicy="no-referrer"
          srcDoc={htmlSource}
          title={`Rendered ${path}`}
          data-testid="file-preview-html"
        />
        <p class="file-preview-note">Scripts, forms and network requests are disabled here.</p>
      </div>
    );
  }

  return <p class="file-notice">There is no preview for this file.</p>;
}

/**
 * Render Markdown and rewrite the links and images that point inside the
 * workspace so they resolve relative to the *file*, not the app's own URL.
 *
 * Relative images are pointed at the raw-bytes endpoint; relative links get a
 * `data-workspace-path` marker the click handler turns into "open that file".
 * Anything absolute (http:, mailto:, data:) is left exactly as rendered. A
 * reference that is relative but escapes the workspace is dropped: leaving the
 * browser to resolve it against the app's own URL would request a path in the app
 * (or, worse, a file the viewer was never allowed to read).
 */
export function renderMarkdownForFile(
  source: string,
  path: string,
  project: string | null,
): string {
  const html = renderMarkdown(source);
  if (typeof document === 'undefined') return html;
  const container = document.createElement('div');
  container.innerHTML = html;
  for (const image of Array.from(container.querySelectorAll('img'))) {
    const source = image.getAttribute('src') ?? '';
    const target = resolveRelativePath(path, source);
    if (target === null) {
      if (isExternalReference(source)) continue;
      // Unresolvable inside the workspace: say so instead of rendering a broken
      // image whose URL would be resolved against the app origin.
      image.replaceWith(container.ownerDocument.createTextNode(`[missing image: ${source}]`));
      continue;
    }
    image.setAttribute('src', workspaceRawUrl(target, project));
    image.setAttribute('loading', 'lazy');
  }
  for (const anchor of Array.from(container.querySelectorAll('a'))) {
    const href = anchor.getAttribute('href') ?? '';
    const target = resolveRelativePath(path, href);
    if (target === null) {
      if (isExternalReference(href)) continue;
      // The link pointed outside the workspace; keep the text, drop the target.
      anchor.removeAttribute('href');
      anchor.setAttribute('title', `${href} is outside the workspace`);
      continue;
    }
    anchor.setAttribute('data-workspace-path', target);
    // Keep the text selectable without navigating the app away from itself.
    anchor.setAttribute('href', `#${target}`);
  }
  return container.innerHTML;
}

/**
 * Wrap workspace HTML in a complete document so it renders the way its author
 * meant it to, with a neutral base style and no way to reach the network.
 */
export function htmlDocument(source: string): string {
  return [
    '<!doctype html>',
    '<html><head><meta charset="utf-8">',
    // Belt and braces: the iframe sandbox already blocks script and origin
    // access, and this CSP also blocks every fetch the document could attempt.
    "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'\">",
    // The document must paint its OWN background and text colour. Without this,
    // `color-scheme: light dark` alone leaves the body transparent with white
    // text, which is invisible against the iframe's white backing - verified in
    // Chrome: `backgroundColor: rgba(0,0,0,0)`, `color: rgb(255,255,255)`.
    // `Canvas`/`CanvasText` are the system colours that always pair up, so an
    // author's own CSS still wins and an unstyled page stays readable in either
    // OS theme.
    '<style>html{color-scheme:light dark;background:Canvas;color:CanvasText}body{margin:0;padding:16px;font:14px/1.6 system-ui,sans-serif}</style>',
    '</head><body>',
    source,
    '</body></html>',
  ].join('\n');
}

/**
 * Delegated copy-button handler, matching the chat's fenced-code affordance.
 * Returns `true` when it handled the click.
 */
function handleCopyClick(target: Element, fallback: string): boolean {
  const button = target.closest<HTMLButtonElement>('[data-copy]');
  if (!button) return false;
  const code = button.closest('.code-block')?.querySelector('code');
  const previous = button.textContent;
  void copyText(code?.textContent ?? fallback).then((ok) => {
    button.textContent = ok ? 'Copied' : 'Copy failed';
    window.setTimeout(() => {
      button.textContent = previous;
    }, 1400);
  });
  return true;
}
