import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, waitFor } from '@testing-library/preact';
import { setAuthToken } from '../api/client';
import { FilePreview, htmlDocument, renderMarkdownForFile } from './FilePreview';

describe('renderMarkdownForFile', () => {
  it('marks relative images for authenticated blob loading without a raw src', () => {
    const html = renderMarkdownForFile('![logo](assets/logo.png)', 'docs/guide.md', null);
    expect(html).toContain('data-workspace-image-path="docs/assets/logo.png"');
    expect(html).toContain('loading="lazy"');
    expect(html).not.toContain('/api/workspace/raw');
    expect(html).not.toContain('src=');
  });

  it('resolves ../ and passes the project through', () => {
    const html = renderMarkdownForFile('![logo](../assets/logo.png)', 'docs/guide.md', 'p_1');
    expect(html).toContain('data-workspace-image-path="assets/logo.png"');
    expect(html).toContain('data-workspace-image-project="p_1"');
    expect(html).not.toContain('/api/workspace/raw');
  });

  it('marks relative links so the viewer can open that file', () => {
    const html = renderMarkdownForFile('[other](other.md)', 'docs/guide.md', null);
    expect(html).toContain('data-workspace-path="docs/other.md"');
  });

  it('leaves absolute URLs exactly as the renderer produced them', () => {
    const html = renderMarkdownForFile(
      '[site](https://example.com/x)\n\n![remote](https://example.com/logo.png)',
      'docs/guide.md',
      null,
    );
    expect(html).toContain('href="https://example.com/x"');
    expect(html).toContain('src="https://example.com/logo.png"');
    expect(html).not.toContain('/api/workspace/raw');
  });

  it('keeps the sanitiser in the loop', () => {
    const html = renderMarkdownForFile(
      '<script>alert(1)</script>\n\n[bad](javascript:alert(1))',
      'docs/guide.md',
      null,
    );
    expect(html).not.toContain('<script>');
    // markdown-it refuses to make a link out of a `javascript:` target at all.
    expect(html).not.toContain('href="javascript');
  });

  it('drops a relative reference that escapes the workspace', () => {
    // Leaving the stray `src` would make the browser resolve it against the app's
    // own URL, which is not a workspace file at all.
    const html = renderMarkdownForFile('![x](../../../etc/passwd)', 'docs/guide.md', null);
    expect(html).not.toContain('/api/workspace/raw');
    expect(html).not.toContain('src=');
    expect(html).toContain('[missing image: ../../../etc/passwd]');
  });

  it('drops the target of a link that escapes the workspace', () => {
    const html = renderMarkdownForFile('[x](../../../etc/passwd)', 'docs/guide.md', null);
    expect(html).not.toContain('href');
    expect(html).toContain('outside the workspace');
  });
});

describe('htmlDocument', () => {
  it('wraps the source in a document that can reach nothing', () => {
    const document = htmlDocument('<h1>hi</h1>');
    expect(document).toContain('<h1>hi</h1>');
    expect(document).toContain("default-src 'none'");
    expect(document).toContain('<meta charset="utf-8">');
  });

  it('paints its own background and text colour', () => {
    // `color-scheme: light dark` on its own left the body transparent with white
    // text, which is invisible against the iframe's backing (verified in Chrome).
    const document = htmlDocument('<h1>hi</h1>');
    expect(document).toContain('background:Canvas');
    expect(document).toContain('color:CanvasText');
  });
});

describe('the HTML preview is a real document, not srcdoc', () => {
  function preview(path: string, project: string | null = null): Element | null {
    const { container } = render(
      <FilePreview path={path} content="<h1>hi</h1>" render="html" project={project} />,
    );
    return container;
  }

  it('points the iframe at the workspace preview URL for that file', () => {
    // `srcdoc` cannot work: it has no base URL (so `./style.css` cannot resolve)
    // and it inherits the app's own CSP, under which an opaque-origin frame may
    // load nothing at all. A real URL is the only way to render a page.
    const frame = preview('docs/site/index.html')?.querySelector('iframe');
    expect(frame).not.toBeNull();
    expect(frame?.getAttribute('src')).toBe('/api/workspace/preview/docs/site/index.html');
    expect(frame?.getAttribute('srcdoc')).toBeNull();
  });

  it('keeps the project in the path so the page’s own relative assets stay scoped', () => {
    // A relative `href="style.css"` drops the query string, so `?project=` scoped
    // only the entry document and every asset of a previewed page 404'd.
    const frame = preview('index.html', 'p_1')?.querySelector('iframe');
    expect(frame?.getAttribute('src')).toBe('/api/workspace/preview-project/p_1/index.html');
  });

  it('encodes each path segment but keeps the separators', () => {
    const frame = preview('my pages/a b#c.html')?.querySelector('iframe');
    expect(frame?.getAttribute('src')).toBe('/api/workspace/preview/my%20pages/a%20b%23c.html');
  });

  it('runs scripts only in an opaque origin: allow-scripts, never allow-same-origin', () => {
    // `allow-scripts` is what makes a previewed page a page. Adding
    // `allow-same-origin` alongside it would hand the page the app's own origin,
    // where it could read the stored bearer token and call `/api`.
    const frame = preview('page.html')?.querySelector('iframe');
    expect(frame?.getAttribute('sandbox')).toBe('allow-scripts');
  });

  it('says the saved page is what is on screen while the editor is dirty', () => {
    const { container } = render(
      <FilePreview path="page.html" content="<h1>draft</h1>" render="html" dirty />,
    );
    expect(container.querySelector('.file-preview-note')?.textContent).toContain('saved page');
  });
});

describe('authenticated workspace media', () => {
  let createDescriptor: PropertyDescriptor | undefined;
  let revokeDescriptor: PropertyDescriptor | undefined;
  const revokeObjectURL = vi.fn();

  beforeEach(() => {
    createDescriptor = Object.getOwnPropertyDescriptor(URL, 'createObjectURL');
    revokeDescriptor = Object.getOwnPropertyDescriptor(URL, 'revokeObjectURL');
    let sequence = 0;
    Object.defineProperty(URL, 'createObjectURL', {
      configurable: true,
      value: vi.fn(() => `blob:workspace-${++sequence}`),
    });
    Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: revokeObjectURL });
    revokeObjectURL.mockClear();
    setAuthToken('browser-test-secret');
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response('media bytes', { status: 200, headers: { 'Content-Type': 'image/png' } })),
    );
  });

  afterEach(() => {
    setAuthToken(null);
    vi.unstubAllGlobals();
    for (const [name, descriptor] of [
      ['createObjectURL', createDescriptor],
      ['revokeObjectURL', revokeDescriptor],
    ] as const) {
      if (descriptor) Object.defineProperty(URL, name, descriptor);
      else delete (URL as unknown as Record<string, unknown>)[name];
    }
  });

  it.each(['image', 'pdf'] as const)('fetches a %s with bearer auth and revokes its object URL', async (kind) => {
    const view = render(
      <FilePreview path={kind === 'pdf' ? 'docs/spec.pdf' : 'assets/logo.png'} content="" render={kind} project="p_1" />,
    );
    const testId = kind === 'pdf' ? 'file-preview-pdf' : 'file-preview-image';
    const element = await waitFor(() => {
      const found = view.container.querySelector(`[data-testid="${testId}"]`);
      if (!found) throw new Error('media not loaded');
      return found;
    });
    expect(element.getAttribute('src')).toBe('blob:workspace-1');

    const fetchMock = vi.mocked(globalThis.fetch);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(String(url)).toContain('/api/workspace/raw?');
    expect(String(url)).toContain('project=p_1');
    expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer browser-test-secret');
    expect(document.body.innerHTML).not.toContain('browser-test-secret');

    view.unmount();
    // preact 11 runs passive-effect cleanup on unmount asynchronously, so the
    // revoke is awaited rather than assumed synchronous.
    await act(async () => { await Promise.resolve(); });
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:workspace-1');
  });

  it('loads a Markdown-relative image without ever assigning the raw src, and reuses it', async () => {
    const view = render(
      <FilePreview path="docs/guide.md" content="![logo](assets/logo.png)" render="markdown" project="p_1" />,
    );
    const image = view.container.querySelector<HTMLImageElement>('img[data-workspace-image-path]')!;
    expect(image.getAttribute('src')).toBeNull();
    await waitFor(() => expect(image.getAttribute('src')).toBe('blob:workspace-1'));

    const fetchMock = vi.mocked(globalThis.fetch);
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(String(url)).toBe('/api/workspace/raw?path=docs%2Fassets%2Flogo.png&project=p_1');
    expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer browser-test-secret');
    expect(document.body.innerHTML).not.toContain('browser-test-secret');

    // The bytes are cached per (project, path): the same image rendered again -
    // another preview, or a transcript row scrolling back into the window - is
    // handed the same object URL instead of being downloaded and decoded again.
    // That is why unmounting does not revoke it; the cache's own cap does.
    view.unmount();
    expect(revokeObjectURL).not.toHaveBeenCalled();
    const again = render(
      <FilePreview path="docs/guide.md" content="![logo](assets/logo.png)" render="markdown" project="p_1" />,
    );
    const second = again.container.querySelector<HTMLImageElement>('img[data-workspace-image-path]')!;
    await waitFor(() => expect(second.getAttribute('src')).toBe('blob:workspace-1'));
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});
