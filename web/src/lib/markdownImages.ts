import { useEffect } from 'preact/hooks';
import { toErrorMessage } from '../api/client';
import { isExternalReference, resolveRelativePath } from './fileKind';
import { holdWorkspaceImage, releaseWorkspaceImage } from './workspaceImages';

/** Attribute the file-preview renderer uses to mark a file-relative image. */
const WORKSPACE_IMAGE_ATTRIBUTE = 'data-workspace-image-path';

/** Class of the link that stands in for an image this deployment cannot load. */
export const EXTERNAL_IMAGE_CLASS = 'markdown-image-link';

/**
 * Replace an `<img>` that points at another origin with a plain link.
 *
 * This app's policy allows no remote image (`img-src 'self' data: blob:`), so an
 * `![screenshot](https://…)` in a reply can never paint: what the reader saw was
 * a broken-image glyph, which reads as "this app is broken" rather than "that
 * file lives somewhere else". A link is both honest and actionable - following it
 * opens the image in its own tab, where the browser's policy applies, not the
 * app's.
 */
export function replaceWithExternalImageLink(image: HTMLImageElement): void {
  const document = image.ownerDocument;
  const href = (image.getAttribute('src') ?? '').trim();
  const alt = (image.getAttribute('alt') ?? '').trim();
  const anchor = document.createElement('a');
  anchor.className = EXTERNAL_IMAGE_CLASS;
  anchor.setAttribute('href', href);
  anchor.setAttribute('target', '_blank');
  anchor.setAttribute('rel', 'noopener noreferrer');
  anchor.setAttribute('title', href);
  anchor.textContent = alt === '' ? `External image: ${href}` : `External image: ${alt}`;
  image.replaceWith(anchor);
}

/**
 * A relative image reference from chat prose, resolved against the workspace.
 *
 * An agent writes `![shot](screenshots/run.png)` meaning the file at the root of
 * the project it is working in, so the reference is workspace-relative and has no
 * file of its own to be relative to. `null` means it escapes the workspace, which
 * is never something the viewer may read.
 */
function workspacePathFor(reference: string): string | null {
  return resolveRelativePath('', reference);
}

/** Show why an image is not drawn, instead of a broken-image glyph. */
function markMissing(image: HTMLImageElement, reason: string): void {
  const document = image.ownerDocument;
  const note = document.createElement('span');
  note.className = 'markdown-image-missing';
  note.textContent = `[${reason}]`;
  image.replaceWith(note);
}

/**
 * Make every image in one rendered Markdown document either work or say why not.
 *
 * Three cases, and none of them is allowed to become a broken image:
 *
 * * a workspace file (marked by the file-preview renderer, or a relative
 *   reference in chat) is fetched through the API with the session's credentials
 *   and shown from a `blob:` URL;
 * * a remote image becomes a link, because the app's policy forbids the request;
 * * a reference that escapes the workspace is replaced by a short note.
 *
 * `html` is a dependency on purpose: the document is swapped wholesale by
 * `dangerouslySetInnerHTML`, so the effect has to run again for the new nodes.
 */
export function useResolvedMarkdownImages(
  root: { current: HTMLElement | null },
  html: string,
  project: string | null,
  enabled: boolean,
): void {
  useEffect(() => {
    const container = root.current;
    if (!enabled || !container) return undefined;
    let cancelled = false;
    /** Refs this effect took, so unmounting the row cannot leak them. */
    const holdings: string[] = [];

    for (const image of Array.from(container.querySelectorAll<HTMLImageElement>('img'))) {
      const marked = image.getAttribute(WORKSPACE_IMAGE_ATTRIBUTE);
      const source = (image.getAttribute('src') ?? '').trim();

      if (marked === null && isExternalReference(source)) {
        if (source !== '') replaceWithExternalImageLink(image);
        continue;
      }

      const target = marked !== null ? marked : workspacePathFor(source);
      if (target === null || target === '') {
        markMissing(image, `image is outside the workspace: ${source}`);
        continue;
      }

      // The raw endpoint in `src` would make the browser issue an unauthenticated
      // request first; the blob URL replaces it once the bytes are here.
      image.removeAttribute('src');
      image.setAttribute('aria-busy', 'true');
      image.setAttribute('loading', 'lazy');
      holdWorkspaceImage(target, project)
        .then((url) => {
          if (cancelled || !container.contains(image)) {
            // The row went away while the bytes were in flight.
            releaseWorkspaceImage(target, project);
            return;
          }
          holdings.push(target);
          image.setAttribute('src', url);
          image.removeAttribute('aria-busy');
        })
        .catch((cause: unknown) => {
          if (cancelled) return;
          image.removeAttribute('aria-busy');
          const message = toErrorMessage(cause);
          image.setAttribute('data-workspace-image-error', message);
          markMissing(image, `could not load ${target}: ${message}`);
        });
    }

    return () => {
      cancelled = true;
      for (const target of holdings) releaseWorkspaceImage(target, project);
    };
  }, [root, html, project, enabled]);
}
