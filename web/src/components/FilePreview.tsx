import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { apiRequestBlob, getAuthToken, toErrorMessage } from '../api/client';
import { workspacePreviewUrl, workspaceRawUrl } from '../api/endpoints';
import { copyText } from '../lib/clipboard';
import { isExternalReference, resolveRelativePath, type FileRender } from '../lib/fileKind';
import { renderMarkdown } from '../lib/markdown';
import { useResolvedMarkdownImages } from '../lib/markdownImages';

export interface FilePreviewProps {
  /** Workspace-relative path of the file being previewed. */
  path: string;
  /** Current contents (the editor's live text when editing, else the loaded file). */
  content: string;
  render: FileRender;
  project?: string | null;
  /** The editor holds unsaved changes, so images/PDFs/HTML on disk are stale. */
  dirty?: boolean;
  /** Open a workspace path that a rendered link pointed at. */
  onOpenFile?: (path: string) => void;
}

interface WorkspaceObjectUrl {
  key: string;
  url: string | null;
  error: string | null;
}

/**
 * Fetch workspace bytes through the authenticated API, then lend the DOM a blob
 * URL. An `img`/`iframe` navigation cannot add an Authorization header itself.
 */
export function useWorkspaceObjectUrl(
  path: string | null,
  project: string | null,
): WorkspaceObjectUrl {
  const identity = JSON.stringify([path, project]);
  const [state, setState] = useState<WorkspaceObjectUrl>({ key: identity, url: null, error: null });

  useEffect(() => {
    if (path === null) {
      setState({ key: identity, url: null, error: null });
      return undefined;
    }
    const controller = new AbortController();
    let cancelled = false;
    let objectUrl: string | null = null;
    setState({ key: identity, url: null, error: null });
    apiRequestBlob(workspaceRawUrl(path, project), { signal: controller.signal })
      .then((blob) => {
        if (cancelled) return;
        objectUrl = URL.createObjectURL(blob);
        if (cancelled) {
          URL.revokeObjectURL(objectUrl);
          return;
        }
        setState({ key: identity, url: objectUrl, error: null });
      })
      .catch((cause: unknown) => {
        if (!cancelled && !controller.signal.aborted) {
          setState({ key: identity, url: null, error: toErrorMessage(cause) });
        }
      });
    return () => {
      cancelled = true;
      controller.abort();
      if (objectUrl !== null) URL.revokeObjectURL(objectUrl);
    };
  }, [path, project, identity]);

  // Effects run after paint. Do not paint the previous file's object URL for one
  // frame while a new identity is waiting for its cleanup effect.
  return state.key === identity ? state : { key: identity, url: null, error: null };
}

/**
 * The rendered half of the file viewer.
 *
 * Everything that can render gets a preview here: Markdown through the same
 * renderer (and sanitiser) the chat uses, HTML as a real page in a sandboxed
 * iframe loaded from its own workspace URL, images and PDFs from the raw-bytes
 * endpoint. Nothing from the workspace is ever inserted into the app's own DOM as
 * markup. The HTML frame runs the page's own scripts - that is the point of
 * previewing a page - but it is an opaque origin served under a policy that
 * blocks foreign network requests and forms. Only relative workspace siblings,
 * inline data and blobs may load; it cannot reach the app's origin, storage or API.
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
  const markdownRef = useRef<HTMLDivElement>(null);
  useResolvedMarkdownImages(markdownRef, markdownHtml, project, render === 'markdown');
  const media = useWorkspaceObjectUrl(render === 'image' || render === 'pdf' ? path : null, project);

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
        ref={markdownRef}
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
    return (
      <div class="file-preview-scroll file-preview-media" role="region" aria-label={`Image ${path}`}>
        {media.error ? (
          <p class="file-notice" role="alert">Could not load {path}: {media.error}</p>
        ) : media.url ? (
          // `alt` names the file: the viewer cannot know what it depicts.
          <img class="file-preview-image" src={media.url} alt={path} data-testid="file-preview-image" />
        ) : (
          <p class="file-notice" role="status">Loading {path}…</p>
        )}
        {dirty ? (
          <p class="file-preview-note">
            The preview shows the saved file; there is no unsaved edit to show.
          </p>
        ) : null}
      </div>
    );
  }

  if (render === 'pdf') {
    return (
      <div class="file-preview-scroll file-preview-media" role="region" aria-label={`PDF ${path}`}>
        {media.error ? (
          <p class="file-notice" role="alert">Could not load {path}: {media.error}</p>
        ) : media.url ? (
          <iframe class="file-preview-pdf" src={media.url} title={path} data-testid="file-preview-pdf" />
        ) : (
          <p class="file-notice" role="status">Loading {path}…</p>
        )}
        <p class="file-preview-note">
          {media.url ? (
            <>
              If the PDF does not render,{' '}
              <a href={media.url} target="_blank" rel="noopener noreferrer">
                open it in a new tab
              </a>
              .
            </>
          ) : (
            'The PDF is fetched with this session’s API credentials.'
          )}
        </p>
      </div>
    );
  }

  if (render === 'html') {
    // A real URL in an iframe, not `srcdoc`. Two reasons, both measured in
    // Chromium: a `srcdoc` frame inherits *this app's* CSP (`default-src 'self'`,
    // no `unsafe-inline` for scripts) and then runs in an opaque origin where
    // `'self'` matches nothing, so its inline scripts, sibling stylesheets and
    // relative images were all refused and the preview came up nearly blank; and
    // a `srcdoc` document has no path, so `./style.css` has nothing to resolve
    // against. Serving the file from `/api/workspace/preview/<path>` gives it
    // both: its own policy (script and relative workspace assets yes, foreign
    // network and forms no, opaque origin) and a base URL in its own directory.
    //
    // `sandbox="allow-scripts"` matches the server's policy and never adds
    // `allow-same-origin`: without that pair the frame would share the app's
    // origin and could read its storage and call `/api`.
    const url = workspacePreviewUrl(path, project);
    // An iframe is a browser navigation: it cannot send the `Authorization`
    // header, and the page's own relative `./style.css` requests cannot carry a
    // query token either. That is fine on the default loopback, token-less
    // deployment, and it is honest to say so rather than render a blank frame
    // that looks like the page is broken.
    const needsToken = getAuthToken() !== null;
    return (
      <div
        class="file-preview-scroll file-preview-media"
        role="region"
        aria-label={`Rendered HTML of ${path}`}
      >
        {needsToken ? (
          <p class="file-notice" data-testid="file-preview-html-needs-token">
            This deployment requires a token, and a rendered page cannot present one. Open{' '}
            <code>{path}</code> from the <strong>Source</strong> tab, or view it inline in the
            workspace.
          </p>
        ) : (
          <iframe
            class="file-preview-html"
            sandbox="allow-scripts"
            referrerPolicy="no-referrer"
            src={url}
            title={`Rendered ${path}`}
            data-testid="file-preview-html"
            // A fresh key per file so reopening a different page never reuses the
            // previous document (an iframe keeps its content when only `src`
            // changes through a same-document hash).
            key={url}
          />
        )}
        <p class="file-preview-note">
          {dirty
            ? 'The preview shows the saved page; save to see your edits rendered.'
            : 'Rendered as an isolated opaque-origin page: scripts and relative workspace assets run/load; remote assets, fetch and forms are blocked.'}
        </p>
      </div>
    );
  }

  return <p class="file-notice">There is no preview for this file.</p>;
}

/**
 * Render Markdown and rewrite the links and images that point inside the
 * workspace so they resolve relative to the *file*, not the app's own URL.
 *
 * Relative images get a marker whose bytes are fetched with API auth and assigned
 * a temporary blob URL; relative links get a `data-workspace-path` marker the
 * click handler turns into "open that file".
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
    // The effect supplies an authenticated blob URL. Leaving the raw endpoint in
    // `src` here would make the browser issue an unauthenticated request first.
    image.removeAttribute('src');
    image.setAttribute('data-workspace-image-path', target);
    image.setAttribute('data-workspace-image-project', project ?? '');
    image.setAttribute('aria-busy', 'true');
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
 *
 * Superseded by `GET /api/workspace/preview/<path>` for the viewer's own HTML
 * preview: a `srcdoc` document has no base URL (so relative assets cannot
 * resolve) and inherits this app's CSP, which leaves it nearly blank. Kept for
 * callers that genuinely have HTML source with no file behind it.
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
