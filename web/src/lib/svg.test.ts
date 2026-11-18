import { afterEach, describe, expect, it, vi } from 'vitest';
import DOMPurify from 'dompurify';
import { hasSvgElement, prepareSvg, resetSvgCache, sanitizeSvg } from './svg';

const SVG = (inner: string): string =>
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 40">${inner}</svg>`;

afterEach(() => {
  resetSvgCache();
});

describe('sanitizeSvg', () => {
  it('keeps what a diagram is made of', () => {
    const drawn = sanitizeSvg(
      SVG(
        '<style>.a{fill:red}</style><g class="node"><rect class="a" width="10" height="10"/>' +
          '<text x="2" y="12">Start</text><path class="flowchart-link" d="M0 0L5 5"/></g>',
      ),
    );
    expect(drawn).toContain('<rect');
    expect(drawn).toContain('Start');
    expect(drawn).toContain('flowchart-link');
    expect(drawn).toContain('<style>');
  });

  it('removes script, event handlers and javascript: links', () => {
    const drawn = sanitizeSvg(
      SVG(
        '<script>steal()</script><rect onload="steal()" onclick="steal()" width="1" height="1"/>' +
          '<a href="javascript:steal()"><text>click</text></a>',
      ),
    );
    expect(drawn).not.toContain('script');
    expect(drawn).not.toContain('onload');
    expect(drawn).not.toContain('onclick');
    expect(drawn).not.toContain('javascript:');
  });

  it('drops foreignObject, and therefore HTML inside SVG', () => {
    // The trade-off behind `htmlLabels: false` in `lib/mermaid.ts`: a diagram
    // whose labels are HTML would render as empty boxes, because this is what
    // the sanitiser does to `foreignObject` - it removes it and its contents.
    const drawn = sanitizeSvg(
      SVG('<foreignObject width="40" height="20"><div><span>Start</span></div></foreignObject>'),
    );
    expect(drawn).not.toContain('foreignObject');
    expect(drawn).not.toContain('Start');
  });

  it('keeps an inline data: image, which a mock may legitimately draw', () => {
    const drawn = sanitizeSvg(SVG('<image href="data:image/png;base64,AAAA" width="4" height="4"/>'));
    expect(drawn).toContain('data:image/png;base64,AAAA');
  });
});

describe('hasSvgElement', () => {
  it('answers whether there is still a drawing after sanitising', () => {
    expect(hasSvgElement(SVG('<rect width="1" height="1"/>'))).toBe(true);
    expect(hasSvgElement('just words')).toBe(false);
    expect(hasSvgElement('')).toBe(false);
  });
});

describe('prepareSvg', () => {
  it('sanitises and probes one document once, however often it is mounted', () => {
    resetSvgCache();
    const sanitize = vi.spyOn(DOMPurify, 'sanitize');
    const source = SVG('<circle cx="5" cy="5" r="4" />');

    const first = prepareSvg(source);
    const again = prepareSvg(source);

    // The same prepared object, so a remount reuses the shadow root's markup too.
    expect(again).toBe(first);
    expect(sanitize).toHaveBeenCalledTimes(1);
    expect(first).toEqual({ html: sanitizeSvg(source), ok: true });
  });

  it('prepares a document with nothing to draw once, and remembers the answer', () => {
    resetSvgCache();
    const sanitize = vi.spyOn(DOMPurify, 'sanitize');
    const source = '<script>steal()</script>';
    expect(prepareSvg(source)).toEqual({ html: '', ok: false });
    expect(prepareSvg(source).ok).toBe(false);
    expect(sanitize).toHaveBeenCalledTimes(1);
  });

  it('keeps a handful of documents, dropping the oldest', () => {
    resetSvgCache();
    const sanitize = vi.spyOn(DOMPurify, 'sanitize');
    // Four is the cap: five distinct drawings must not all stay prepared.
    const sources = Array.from({ length: 5 }, (_, index) =>
      SVG(`<text>${index}</text>`),
    );
    for (const source of sources) prepareSvg(source);
    expect(sanitize).toHaveBeenCalledTimes(5);

    for (const source of sources.slice(1)) prepareSvg(source);
    expect(sanitize).toHaveBeenCalledTimes(5);

    // The first one was evicted, so asking for it again is a real parse.
    prepareSvg(sources[0] ?? '');
    expect(sanitize).toHaveBeenCalledTimes(6);
  });
});
