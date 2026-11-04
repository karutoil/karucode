/**
 * Save text to disk through a temporary object URL.
 *
 * One helper rather than an anchor in each component: the URL has to be revoked
 * after the click or the blob is retained for the life of the document, and the
 * anchor has to be in the tree for a synthetic click to be trusted by every
 * browser that matters.
 */
export function downloadText(filename: string, text: string, mime = 'text/plain'): void {
  const url = URL.createObjectURL(new Blob([text], { type: `${mime};charset=utf-8` }));
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  anchor.rel = 'noopener';
  anchor.style.display = 'none';
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  // Revoking immediately can race the browser's own read of the URL in some
  // builds; a tick is enough and does not leak for long.
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** A filename that cannot escape its directory or upset a filesystem. */
export function safeFilename(name: string, fallback: string): string {
  const cleaned = name
    .trim()
    .replace(/[\\/:*?"<>|\u0000-\u001f]/g, '-')
    .replace(/\s+/g, '-')
    .replace(/-+/g, '-')
    .replace(/^[-.]+|[-.]+$/g, '')
    .slice(0, 80);
  return cleaned === '' ? fallback : cleaned;
}
