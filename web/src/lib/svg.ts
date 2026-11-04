import DOMPurify from 'dompurify';

/**
 * Sanitise one SVG document for inline insertion into the app's own DOM.
 *
 * Inline SVG is the one artifact kind that is **not** isolated: it shares the
 * app's origin, so a `<script>` inside it would run with the app's authority,
 * an `onload=` attribute would fire, and an `<a href="javascript:">` would be a
 * link into the app. DOMPurify's SVG profile removes all three, which is why the
 * inline path exists at all - an SVG rendered through a frame would need a
 * document per diagram and could not inherit the app's theme.
 *
 * `foreignObject` is allowed (it is how an SVG mock carries HTML), and DOMPurify
 * sanitises its contents with the HTML profile as well.
 */
export function sanitizeSvg(source: string): string {
  return DOMPurify.sanitize(source, {
    USE_PROFILES: { svg: true, svgFilters: true, html: true },
    ADD_ATTR: ['xmlns', 'xmlns:xlink', 'preserveAspectRatio', 'dominant-baseline'],
    FORBID_TAGS: ['script', 'base', 'link', 'meta'],
    FORBID_ATTR: ['onload', 'onerror', 'onclick', 'onmouseover'],
  });
}

/**
 * Is there still an `<svg>` element after sanitising?
 *
 * The sanitiser is allowed to return a document whose drawing was removed - an
 * SVG that was nothing but a script, for instance - and a card that shows an
 * empty box with no explanation is worse than one that says what happened.
 */
export function hasSvgElement(sanitized: string): boolean {
  if (typeof document === 'undefined') return /<svg[\s>]/i.test(sanitized);
  const probe = document.createElement('div');
  probe.innerHTML = sanitized;
  return probe.querySelector('svg') !== null;
}
