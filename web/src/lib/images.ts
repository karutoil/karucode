import type { ImageAttachment } from '../api/types';
import { formatBytes } from './format';

/**
 * Media types a browser renders safely in an `<img>` and the vision APIs accept.
 *
 * The list is an allowlist rather than "whatever the server said": a media type
 * is interpolated into a `data:` URL, which is a place a hostile value has no
 * business being. Anything else is shown as a named refusal instead of an image.
 */
const PREVIEWABLE = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp']);

/** `true` when this attachment can be shown as a thumbnail. */
export function canPreviewImage(image: ImageAttachment): boolean {
  return PREVIEWABLE.has((image.media_type ?? '').toLowerCase()) && (image.data ?? '').length > 0;
}

/**
 * How many built data URLs are kept, per attachment object.
 *
 * Eight is roughly one tool result's worth: the screenshot/read tools return a
 * handful of images, and one card cannot show more than that at once. The cache
 * exists because the URL is a copy of the whole image: a running tool card
 * renders per streamed delta, and four 2 MB screenshots were ~11 MB of string
 * concatenation per delta, which was then handed to Preact to diff as an
 * attribute. An entry is dropped oldest-first (the same idiom
 * `artifactPreview.ts` uses), which is deliberately lossy - a miss costs one
 * concatenation, never a request.
 *
 * Keyed by the attachment object rather than by its bytes: comparing the object
 * is one pointer test, while building a key out of `data` would copy the very
 * string this is here to avoid copying.
 */
const MAX_CACHED = 8;

const cache = new Map<ImageAttachment, string>();

/** The `src` for an `<img>`, or `null` when the type must not be rendered. */
export function imageDataUrl(image: ImageAttachment): string | null {
  // The cache is checked before the previewable test, so a hit reads nothing at
  // all: an attachment that was previewable when it was built stays that way.
  const cached = cache.get(image);
  if (cached !== undefined) return cached;
  if (!canPreviewImage(image)) return null;
  const url = `data:${image.media_type.toLowerCase()};base64,${image.data}`;
  cache.set(image, url);
  while (cache.size > MAX_CACHED) {
    const oldest = cache.keys().next();
    if (oldest.done) break;
    cache.delete(oldest.value);
  }
  return url;
}

/** Drop everything remembered. Used by the tests, which must not leak state. */
export function resetImageCache(): void {
  cache.clear();
}

/** Caption under a thumbnail: the file it came from and how big it is. */
export function imageLabel(image: ImageAttachment, fallback = 'image'): string {
  const name = (image.name ?? '').trim() || fallback;
  const size = formatBytes(image.bytes);
  return size === '' ? name : `${name} · ${size}`;
}
