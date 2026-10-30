import { describe, expect, it } from 'vitest';
import { canPreviewImage, imageDataUrl, imageLabel } from './images';
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
