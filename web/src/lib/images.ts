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

/** The `src` for an `<img>`, or `null` when the type must not be rendered. */
export function imageDataUrl(image: ImageAttachment): string | null {
  if (!canPreviewImage(image)) return null;
  return `data:${image.media_type.toLowerCase()};base64,${image.data}`;
}

/** Caption under a thumbnail: the file it came from and how big it is. */
export function imageLabel(image: ImageAttachment, fallback = 'image'): string {
  const name = (image.name ?? '').trim() || fallback;
  const size = formatBytes(image.bytes);
  return size === '' ? name : `${name} · ${size}`;
}
