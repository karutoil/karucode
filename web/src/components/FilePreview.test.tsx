import { describe, expect, it } from 'vitest';
import { htmlDocument, renderMarkdownForFile } from './FilePreview';

describe('renderMarkdownForFile', () => {
  it('points relative images at the raw-bytes endpoint for that file', () => {
    const html = renderMarkdownForFile('![logo](assets/logo.png)', 'docs/guide.md', null);
    expect(html).toContain('src="/api/workspace/raw?path=docs%2Fassets%2Flogo.png"');
    expect(html).toContain('loading="lazy"');
  });

  it('resolves ../ and passes the project through', () => {
    const html = renderMarkdownForFile('![logo](../assets/logo.png)', 'docs/guide.md', 'p_1');
    // `&` is escaped in the attribute, which is exactly what the browser parses
    // back to a query separator.
    expect(html).toContain('/api/workspace/raw?path=assets%2Flogo.png&amp;project=p_1');
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
