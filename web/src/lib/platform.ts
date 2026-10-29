/**
 * `Ctrl`/`⌘` for the current platform, so every shortcut hint in the UI
 * (header titles, keycaps, tooltips) is never wrong.
 *
 * Kept in one place: the rail, the sidebar and the header all print it.
 */
export function modKey(): string {
  if (typeof navigator === 'undefined') return 'Ctrl';
  const platform = `${navigator.platform ?? ''} ${navigator.userAgent ?? ''}`;
  return /mac|iphone|ipad|ipod/i.test(platform) ? '⌘' : 'Ctrl';
}
