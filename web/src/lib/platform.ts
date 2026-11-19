/**
 * Is this an Apple platform?
 *
 * Two different questions depend on the answer, which is why it is one predicate:
 *
 * 1. Which modifier the hints print (`⌘` or `Ctrl`). The rail, the sidebar and
 *    the header all print it, so it lives in one place.
 * 2. Which modifier the shell must leave to a focused text field. A bare
 *    `Ctrl`+letter is an emacs editing chord there - `Ctrl+B` cursor-left,
 *    `Ctrl+K` kill-to-end-of-line, `Ctrl+J` next line - while the app's own
 *    chords are meta-based. On Windows and Linux `Ctrl` *is* the app modifier,
 *    and `Ctrl+K` from the composer is expected to open the palette.
 */
export function isApplePlatform(): boolean {
  if (typeof navigator === 'undefined') return false;
  const platform = `${navigator.platform ?? ''} ${navigator.userAgent ?? ''}`;
  return /mac|iphone|ipad|ipod/i.test(platform);
}

/** `Ctrl`/`⌘` for the current platform, so every shortcut hint is never wrong. */
export function modKey(): string {
  return isApplePlatform() ? '⌘' : 'Ctrl';
}
