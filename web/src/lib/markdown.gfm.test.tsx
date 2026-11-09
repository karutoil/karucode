import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, waitFor } from '@testing-library/preact';
import { Markdown } from '../components/Markdown';
import { dropWorkspaceImages } from './workspaceImages';
import { resetAppState } from '../test/fixtures';
import * as store from '../state/store';

/**
 * GitHub's Markdown in the chat renderer: the block extensions (task lists,
 * footnotes, tables, strikethrough), autolinks, and the three ways an image can
 * arrive.
 */
describe('GFM rendering', () => {
  beforeEach(() => {
    resetAppState();
    dropWorkspaceImages();
  });

  afterEach(() => {
    cleanup();
    dropWorkspaceImages();
    vi.unstubAllGlobals();
    resetAppState();
  });

  function renderMarkdown(text: string): HTMLElement {
    return render(<Markdown text={text} />).container as HTMLElement;
  }

  it('renders a GFM table', () => {
    const container = renderMarkdown('| a | b |\n| --- | --- |\n| 1 | 2 |');
    expect(container.querySelector('table th')?.textContent).toBe('a');
    expect(container.querySelectorAll('tbody td')).toHaveLength(2);
  });

  it('renders strikethrough', () => {
    const container = renderMarkdown('~~gone~~');
    expect(container.querySelector('s')?.textContent).toBe('gone');
  });

  it('autolinks a bare URL and opens it in a new tab', () => {
    const container = renderMarkdown('see https://example.com/docs for more');
    const anchor = container.querySelector('a');
    expect(anchor?.getAttribute('href')).toBe('https://example.com/docs');
    expect(anchor?.getAttribute('target')).toBe('_blank');
    expect(anchor?.getAttribute('rel')).toBe('noopener noreferrer');
  });

  it('renders a task list as read-only checkboxes', () => {
    const container = renderMarkdown('- [x] shipped\n- [ ] pending');
    const items = Array.from(container.querySelectorAll('li.task-list-item'));
    expect(items).toHaveLength(2);
    const boxes = Array.from(container.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'));
    expect(boxes).toHaveLength(2);
    expect(boxes[0]?.hasAttribute('checked')).toBe(true);
    expect(boxes[1]?.hasAttribute('checked')).toBe(false);
    // A rendered message is a document: the box must not pretend to be a control.
    for (const box of boxes) {
      expect(box.disabled).toBe(true);
    }
    expect(container.querySelector('li.task-list-item')?.textContent).toContain('shipped');
  });

  it('renders footnotes with in-page references', () => {
    const container = renderMarkdown('A claim[^1]\n\n[^1]: the source');
    const ref = container.querySelector('.footnote-ref a');
    expect(ref?.getAttribute('href')).toBe('#fn1');
    // A fragment is a place in this document: opening it in a tab would show a
    // second copy of the page instead of the note.
    expect(ref?.getAttribute('target')).toBeNull();
    const item = container.querySelector('.footnotes-list .footnote-item');
    expect(item?.textContent).toContain('the source');
    expect(item?.querySelector('a.footnote-backref')?.getAttribute('href')).toBe('#fnref1');
  });

  it('never lets a non-checkbox form control through the sanitiser', () => {
    // Raw HTML is off, so this is literal text; the allow-list is what makes that
    // double-safe if it were ever turned on.
    const container = renderMarkdown('<input type="text" value="secret"> <form></form>');
    expect(container.querySelector('input')).toBeNull();
    expect(container.querySelector('form')).toBeNull();
    expect(container.textContent).toContain('<input type="text"');
  });

  it('fetches a workspace-relative image with the session credentials', async () => {
    store.currentProjectId.value = 'p_1';
    const objectUrl = vi.fn(() => 'blob:shot');
    Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: objectUrl });
    Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: () => undefined });
    const fetchMock = vi.fn(
      async () => new Response('png bytes', { status: 200, headers: { 'Content-Type': 'image/png' } }),
    );
    vi.stubGlobal('fetch', fetchMock);

    const container = renderMarkdown('![a screenshot](shots/run.png)');
    const image = container.querySelector<HTMLImageElement>('img');
    expect(image).not.toBeNull();
    // The raw endpoint in `src` would be an unauthenticated request first.
    expect(image?.getAttribute('src')).toBeNull();
    await waitFor(() => expect(image?.getAttribute('src')).toBe('blob:shot'));
    expect(image?.getAttribute('alt')).toBe('a screenshot');

    const [url, init] = vi.mocked(globalThis.fetch).mock.calls[0]!;
    expect(String(url)).toBe('/api/workspace/raw?path=shots%2Frun.png&project=p_1');
    expect(new Headers(init?.headers).get('Authorization')).toBeNull();
  });

  it('turns a remote image into a link, because the policy forbids the request', () => {
    const container = renderMarkdown('![diagram](https://example.com/d.png)');
    // No broken-image glyph: the reader gets something they can open.
    expect(container.querySelector('img')).toBeNull();
    const anchor = container.querySelector('a.markdown-image-link');
    expect(anchor?.getAttribute('href')).toBe('https://example.com/d.png');
    expect(anchor?.getAttribute('target')).toBe('_blank');
    expect(anchor?.textContent).toContain('diagram');
  });

  it('says why an image outside the workspace is not drawn', () => {
    const container = renderMarkdown('![x](../../../etc/passwd)');
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('.markdown-image-missing')?.textContent).toContain(
      'outside the workspace',
    );
  });
});
