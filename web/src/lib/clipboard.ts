/** Copies text, falling back to a hidden textarea when the Clipboard API is unavailable. */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && typeof navigator.clipboard.writeText === 'function') {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Fall through to the legacy path (e.g. non-secure contexts).
  }

  // The only path on a plain-HTTP origin — the documented LAN deployment — so it
  // has to keep the page usable. Two things it must not do:
  //
  // * copy from an unfocused field: `execCommand('copy')` acts on the focused
  //   control and `select()` alone does not focus one;
  // * leave focus on `<body>`: removing the scratch element that held focus is
  //   how the composer used to go dead until the user clicked it again.
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const textarea = document.createElement('textarea');
  try {
    textarea.value = text;
    textarea.setAttribute('readonly', '');
    textarea.style.position = 'fixed';
    textarea.style.top = '0';
    textarea.style.left = '0';
    textarea.style.opacity = '0';
    document.body.appendChild(textarea);
    textarea.focus();
    // iOS ignores `select()` on a readonly field but honours an explicit range.
    textarea.setSelectionRange(0, text.length);
    textarea.select();
    return document.execCommand('copy');
  } catch {
    return false;
  } finally {
    textarea.remove();
    if (previous && previous.isConnected) previous.focus();
  }
}
