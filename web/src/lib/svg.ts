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

/** What a card needs to draw one SVG document, computed together. */
export interface PreparedSvg {
  /** The sanitised document, ready for `innerHTML`. */
  html: string;
  /** Whether anything drawable survived the sanitiser. */
  ok: boolean;
}

/**
 * How many prepared documents stay cached.
 *
 * One mount costs three parses of the document (the sanitise, the
 * `hasSvgElement` probe's own `innerHTML`, and the shadow root's), and the
 * transcript unmounts rows that leave its window - so scrolling a diagram out and
 * back used to re-do all three for an 800 KB drawing. Four is enough for the
 * diagrams one screen of transcript holds; each entry keeps the source and the
 * sanitised copy, so the cache stays small.
 */
const MAX_CACHED = 4;

/** Past this the cache holds the newest document only: a huge mock is not worth four. */
const MAX_CACHED_CHARS = 4 * 1024 * 1024;

const cache = new Map<string, PreparedSvg>();
let cachedChars = 0;

/**
 * Sanitise one SVG document and report whether it still draws, once per source.
 *
 * Keyed by the source itself (a string compare, and Preact hands back the same
 * instance across re-renders) because a hash of an 800 KB document would cost as
 * much as the parse it replaces. Text is immutable, so a hit can share the
 * prepared object between the two cards showing the same drawing.
 *
 * Two budgets, and the second one is the hard one: at most {@link MAX_CACHED}
 * documents, and at most {@link MAX_CACHED_CHARS} characters of source plus
 * sanitised copy in total. A single document that alone exceeds the byte budget
 * is drawn but never remembered; once the newest document is the only one left,
 * the eviction loop *does* drop it, so the total can never sit above the budget
 * because of the entry that just arrived.
 */
export function prepareSvg(source: string): PreparedSvg {
  const cached = cache.get(source);
  if (cached !== undefined) return cached;
  const html = sanitizeSvg(source);
  const prepared: PreparedSvg = { html, ok: hasSvgElement(html) };
  const size = source.length + html.length;
  if (size > MAX_CACHED_CHARS) return prepared;
  cache.set(source, prepared);
  cachedChars += size;
  while (cache.size > MAX_CACHED || cachedChars > MAX_CACHED_CHARS) {
    const oldest = cache.keys().next();
    if (oldest.done) break;
    const entry = cache.get(oldest.value);
    cache.delete(oldest.value);
    cachedChars -= oldest.value.length + (entry?.html.length ?? 0);
  }
  return prepared;
}

/** Drop everything prepared. Used by the tests, which must not leak state. */
export function resetSvgCache(): void {
  cache.clear();
  cachedChars = 0;
}
