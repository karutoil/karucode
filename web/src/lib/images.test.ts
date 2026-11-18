import { describe, expect, it } from 'vitest';
import { canPreviewImage, imageDataUrl, imageLabel, resetImageCache } from './images';
import type { ImageAttachment } from '../api/types';

function image(overrides: Partial<ImageAttachment> = {}): ImageAttachment {
  return {
    media_type: 'image/png',
    data: 'iVBORw0KGgo=',
    bytes: 8,
    name: 'assets/shot.png',
    ...overrides,
  };
}

/**
 * An attachment that counts how many times its bytes are read.
 *
 * A `data:` URL is a fresh string every time it is built, so "was it built
 * again" cannot be asked of the string itself (two equal strings are
 * indistinguishable). What *is* observable is whether the bytes had to be
 * touched at all: a cached image is served without ever reading `data`.
 */
function countingImage(data = 'iVBORw0KGgo='): {
  attachment: ImageAttachment;
  reads: () => number;
} {
  const attachment = image({ data, bytes: data.length });
  let reads = 0;
  Object.defineProperty(attachment, 'data', {
    configurable: true,
    get() {
      reads += 1;
      return data;
    },
  });
  return { attachment, reads: () => reads };
}

describe('image help', () => {
  it('builds a data URL for a previewable type', () => {
    expect(imageDataUrl(image())).toBe('data:image/png;base64,iVBORw0KGgo=');
    expect(imageDataUrl(image({ media_type: 'IMAGE/WEBP' }))).toBe('data:image/webp;base64,iVBORw0KGgo=');
  });

  it('refuses a type it must not interpolate into a data URL', () => {
    for (const media_type of ['text/html', 'image/svg+xml', 'image/tiff', '']) {
      const attachment = image({ media_type });
      expect(canPreviewImage(attachment)).toBe(false);
      expect(imageDataUrl(attachment)).toBeNull();
    }
    // A type that is fine but has no bytes is not previewable either.
    expect(imageDataUrl(image({ data: '' }))).toBeNull();
  });

  it('labels an image with its source and size', () => {
    expect(imageLabel(image())).toBe('assets/shot.png · 8 B');
    expect(imageLabel(image({ name: null, bytes: 2048 }))).toBe('image · 2.0 KB');
    expect(imageLabel(image({ name: '  ', bytes: 0 }), 'attachment')).toBe('attachment · 0 B');
    expect(imageLabel(image({ name: 'x.png', bytes: 0 }))).toBe('x.png · 0 B');
  });
});

describe('imageDataUrl across renders', () => {
  it('builds one data URL per image instead of one per call', () => {
    const { attachment, reads } = countingImage('iVBORw0KGgoAAAANSUhEUg');
    const first = imageDataUrl(attachment);
    const afterFirstBuild = reads();

    // A streaming tool card renders per delta; four more renders must not touch
    // the bytes of a 2 MB screenshot again.
    const rendered = [1, 2, 3, 4].map(() => imageDataUrl(attachment));

    expect(rendered).toEqual([first, first, first, first]);
    expect(reads()).toBe(afterFirstBuild);
  });

  it('keeps eight images, so a ninth distinct one drops the oldest', () => {
    resetImageCache();
    const oldest = countingImage('first-image');
    imageDataUrl(oldest.attachment);
    const afterFirstBuild = oldest.reads();
    expect(afterFirstBuild).toBeGreaterThan(0);

    for (let index = 0; index < 7; index += 1) {
      imageDataUrl(countingImage(`image-${index}`).attachment);
    }
    // Still inside the cache: served without reading its bytes.
    imageDataUrl(oldest.attachment);
    expect(oldest.reads()).toBe(afterFirstBuild);

    imageDataUrl(countingImage('ninth-image').attachment);
    imageDataUrl(oldest.attachment);
    expect(oldest.reads()).toBeGreaterThan(afterFirstBuild);
  });

  it('does not cache a refusal', () => {
    const attachment = image({ media_type: 'image/tiff' });
    expect(imageDataUrl(attachment)).toBeNull();
    expect(imageDataUrl(attachment)).toBeNull();
  });
});
