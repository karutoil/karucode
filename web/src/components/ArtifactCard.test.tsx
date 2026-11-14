import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { Markdown } from './Markdown';
import { invalidateArtifact, publishArtifact, resetArtifactCache } from '../lib/artifactPreview';
import { renderMermaid } from '../lib/mermaid';
import { setAuthToken } from '../api/client';
import * as store from '../state/store';

vi.mock('../lib/artifactPreview', () => ({
  publishArtifact: vi.fn(async () => '/api/artifacts/abc123?capability=scoped-artifact-read'),
  invalidateArtifact: vi.fn(),
  resetArtifactCache: vi.fn(),
}));

vi.mock('../lib/mermaid', () => ({
  renderMermaid: vi.fn(async () => '<svg id="diagram"><g><rect /></g></svg>'),
}));

const HTML_MOCK =
  '<!doctype html><html><body><h1>Buy the thing</h1></body></html>';
const MERMAID = 'graph TD\n  a[Start]-->b[Stop]';

/** A fenced block, the way the model writes one. */
function fence(info: string, body: string): string {
  return `\`\`\`${info}\n${body}\n\`\`\``;
}

function card(container: Element): HTMLElement {
  const found = container.querySelector<HTMLElement>('[data-testid="artifact-card"]');
  if (!found) throw new Error('no artifact card rendered');
  return found;
}

beforeEach(() => {
  vi.mocked(publishArtifact).mockResolvedValue('/api/artifacts/abc123?capability=scoped-artifact-read');
  vi.mocked(renderMermaid).mockResolvedValue('<svg id="diagram"><g><rect /></g></svg>');
});

afterEach(() => {
  resetArtifactCache();
});

describe('an artifact fence', () => {
  it('renders a card instead of a code block when the author asked for one', () => {
    const { container } = render(<Markdown text={fence('artifact html title="Signup form"', HTML_MOCK)} />);
    const element = card(container);
    expect(element.getAttribute('data-kind')).toBe('html');
    expect(element.getAttribute('data-view')).toBe('preview');
    expect(element.textContent).toContain('HTML');
    expect(element.textContent).toContain('Signup form');
    expect(container.querySelector('.code-block')).toBeNull();
  });

  it('keeps a small snippet as a plain code block', () => {
    const { container } = render(<Markdown text={fence('html', '<div>hi</div>')} />);
    expect(container.querySelector('[data-testid="artifact-card"]')).toBeNull();
    expect(container.querySelector('.code-block')).not.toBeNull();
  });

  it('renders nothing while the message is still streaming', () => {
    const { container } = render(
      <Markdown text={fence('artifact html', HTML_MOCK)} artifacts={false} />,
    );
    expect(container.querySelector('[data-testid="artifact-card"]')).toBeNull();
    expect(container.querySelector('.code-block')).not.toBeNull();
  });

  it('reads the metadata header block, and strips it from the source', async () => {
    const text = fence('artifact', 'title: Checkout\ntype: html\nheight: 300\n---\n<div>hi</div>');
    const { container } = render(<Markdown text={text} />);
    const element = card(container);
    expect(element.textContent).toContain('Checkout');
    expect(publishArtifact).toHaveBeenCalledWith('<div>hi</div>\n', 'dark');
  });
});

describe('an HTML artifact', () => {
  it('is published once and drawn in a sandboxed frame', async () => {
    const { container } = render(<Markdown text={fence('artifact html', HTML_MOCK)} />);
    await waitFor(() => expect(container.querySelector('iframe')).not.toBeNull());
    expect(publishArtifact).toHaveBeenCalledTimes(1);
    const frame = container.querySelector('iframe')!;
    expect(frame.getAttribute('src')).toBe('/api/artifacts/abc123?capability=scoped-artifact-read');
    expect(frame.getAttribute('sandbox')).toBe('allow-scripts');
    expect(frame.getAttribute('sandbox')).not.toContain('allow-same-origin');
    expect(frame.getAttribute('title')).toContain('rendered as an isolated page');
  });

  it('invalidates a cached capability and publishes fresh source on Render again', async () => {
    const fresh = '/api/artifacts/fresh456?capability=new-read';
    vi.mocked(publishArtifact)
      .mockResolvedValueOnce('/api/artifacts/abc123?capability=dead-read')
      .mockResolvedValueOnce(fresh);
    setAuthToken('master-secret-not-for-artifacts');
    const { container } = render(<Markdown text={fence('artifact html', HTML_MOCK)} />);
    await waitFor(() => expect(container.querySelector('iframe')).not.toBeNull());

    fireEvent.click(screen.getByRole('button', { name: 'Render this artifact again' }));
    await waitFor(() => expect(container.querySelector('iframe')?.getAttribute('src')).toBe(`${fresh}&v=1`));
    expect(invalidateArtifact).toHaveBeenCalledWith(HTML_MOCK + '\n', 'dark');
    expect(publishArtifact).toHaveBeenCalledTimes(2);
    expect(document.body.innerHTML).not.toContain('master-secret-not-for-artifacts');
    setAuthToken(null);
  });

  it('says it is isolated, and can show the source instead', async () => {
    const { container } = render(<Markdown text={fence('artifact html', HTML_MOCK)} />);
    expect(container.textContent).toContain('isolated frame');
    const source = screen.getByRole('button', { name: 'Source' });
    fireEvent.click(source);
    expect(card(container).getAttribute('data-view')).toBe('source');
    expect(container.querySelector('[data-testid="artifact-source"]')).not.toBeNull();
    expect(container.querySelector('iframe')).toBeNull();
    // The "isolated frame" note belongs to the preview, not to the source.
    expect(container.textContent).not.toContain('isolated frame');
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    expect(card(container).getAttribute('data-view')).toBe('preview');
  });

  it('sizes the frame to the document\u2019s own report, and ignores everything else', async () => {
    const { container } = render(<Markdown text={fence('artifact html', HTML_MOCK)} />);
    const frame = await waitFor(() => {
      const found = container.querySelector<HTMLIFrameElement>('iframe');
      if (!found) throw new Error('not yet');
      return found;
    });
    const inner = frame.contentWindow;
    expect(inner).not.toBeNull();
    const report = (height: number, source: Window | null): void => {
      window.dispatchEvent(
        new MessageEvent('message', { data: { type: 'karucode-artifact', height }, source }),
      );
    };

    report(180, inner);
    await waitFor(() => expect(frame.style.height).toBe('180px'));

    // A message from anywhere else is not this frame's report.
    report(600, window);
    await new Promise((resolve) => window.setTimeout(resolve, 20));
    expect(frame.style.height).toBe('180px');

    // A nonsense height is clamped rather than trusted.
    report(99_999, inner);
    await waitFor(() => expect(frame.style.height).toBe('720px'));
  });

  it('offers the source when the frame cannot be published', async () => {
    vi.mocked(publishArtifact).mockRejectedValue(new Error('server said no'));
    const { container } = render(<Markdown text={fence('artifact html', HTML_MOCK)} />);
    const failure = await waitFor(() => {
      const found = container.querySelector<HTMLElement>('[data-testid="artifact-error"]');
      if (!found) throw new Error('not yet');
      return found;
    });
    expect(failure.textContent).toContain('server said no');
    fireEvent.click(screen.getByRole('button', { name: 'Show source' }));
    expect(container.querySelector('[data-testid="artifact-source"]')).not.toBeNull();
  });
});

describe('an SVG artifact', () => {
  it('is drawn inline with its scripts and event handlers removed', () => {
    const svg =
      '<svg viewBox="0 0 10 10" onload="alert(1)"><script>alert(2)</script>' +
      '<circle cx="5" cy="5" r="4" /></svg>';
    const { container } = render(<Markdown text={fence('svg', svg)} />);
    const host = container.querySelector<HTMLElement>('[data-testid="artifact-svg"]');
    expect(host).not.toBeNull();
    const drawn = host?.shadowRoot;
    expect(drawn?.querySelector('circle')).not.toBeNull();
    expect(drawn?.querySelector('script')).toBeNull();
    expect(drawn?.querySelector('svg')?.getAttribute('onload')).toBeNull();
  });

  it('keeps its own CSS inside a shadow root, so it cannot restyle the app', () => {
    const svg =
      '<svg viewBox="0 0 10 10" width="10" height="10"><style>svg{display:none}</style>' +
      '<circle cx="5" cy="5" r="4" /></svg>';
    const { container } = render(<Markdown text={fence('svg', svg)} />);
    const host = container.querySelector<HTMLElement>('[data-testid="artifact-svg"]');
    // The stylesheet is in the shadow root with the drawing, not in the document.
    expect(container.querySelector('style')).toBeNull();
    expect(host?.shadowRoot?.querySelector('style')).not.toBeNull();
    expect(host?.shadowRoot?.querySelector('circle')).not.toBeNull();
  });

  it('says so when sanitising leaves nothing to draw', () => {
    // A fence that declared itself SVG but carries no drawing at all: the
    // sanitiser is not what removed it, there was never an `svg` element to draw.
    const { container } = render(<Markdown text={fence('artifact svg', 'just words')} />);
    expect(container.querySelector('[data-testid="artifact-error"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="artifact-svg"]')).toBeNull();
  });
});

describe('a Mermaid artifact', () => {
  it('renders the diagram in the app theme', async () => {
    const { container } = render(<Markdown text={fence('mermaid', MERMAID)} />);
    await waitFor(() => {
      const host = container.querySelector<HTMLElement>('[data-testid="artifact-mermaid"]');
      expect(host?.shadowRoot?.querySelector('svg')).not.toBeNull();
    });
    expect(renderMermaid).toHaveBeenCalledWith(MERMAID + '\n', expect.stringMatching(/light|dark/));
  });

  it('explains a parse failure and offers the source', async () => {
    vi.mocked(renderMermaid).mockRejectedValue(new Error('Parse error on line 2'));
    const { container } = render(<Markdown text={fence('mermaid', MERMAID)} />);
    const failure = await waitFor(() => {
      const found = container.querySelector<HTMLElement>('[data-testid="artifact-error"]');
      if (!found) throw new Error('not yet');
      return found;
    });
    expect(failure.textContent).toContain('Parse error on line 2');
    fireEvent.click(screen.getByRole('button', { name: 'Show source' }));
    expect(card(container).getAttribute('data-view')).toBe('source');
    expect(container.textContent).toContain('graph TD');
  });
});

describe('a JSON artifact', () => {
  it('pretty-prints a minified payload', () => {
    const source = JSON.stringify({ a: 1, b: [2, 3], pad: 'x'.repeat(200) });
    const { container } = render(<Markdown text={fence('json', source)} />);
    const drawn = container.querySelector('[data-testid="artifact-json"]');
    // The card opens on the source for JSON, so the preview has to be asked for.
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    expect(container.querySelector('[data-testid="artifact-json"]')).not.toBeNull();
    expect(drawn).toBeNull();
    expect(container.querySelector('[data-testid="artifact-json"]')?.textContent).toContain('"a": 1');
  });

  it('says what is wrong with invalid JSON', () => {
    const { container } = render(<Markdown text={fence('artifact json', '{ "a": }')} />);
    const failure = container.querySelector('[data-testid="artifact-error"]');
    expect(failure?.textContent).toContain('not valid JSON');
  });

  it('leaves a broken JSON fence as code when the author did not ask for a card', () => {
    const broken = `{"a": ${'x'.repeat(300)}`;
    const { container } = render(<Markdown text={fence('json', broken)} />);
    expect(container.querySelector('[data-testid="artifact-card"]')).toBeNull();
    expect(container.querySelector('.code-block')).not.toBeNull();
  });
});

describe('a path image artifact', () => {
  it('uses bearer-authenticated bytes and revokes its blob URL', async () => {
    const createDescriptor = Object.getOwnPropertyDescriptor(URL, 'createObjectURL');
    const revokeDescriptor = Object.getOwnPropertyDescriptor(URL, 'revokeObjectURL');
    const revoke = vi.fn();
    Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: () => 'blob:artifact-image' });
    Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: revoke });
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) =>
      new Response('png', { status: 200, headers: { 'Content-Type': 'image/png' } }),
    );
    vi.stubGlobal('fetch', fetchMock);
    setAuthToken('image-master-secret');
    store.currentProjectId.value = 'p_images';

    const view = render(<Markdown text={fence('artifact image', 'assets/dot.png')} />);
    await waitFor(() => expect(view.container.querySelector('[data-testid="artifact-image"]')).not.toBeNull());
    const image = view.container.querySelector('[data-testid="artifact-image"]')!;
    expect(image.getAttribute('src')).toBe('blob:artifact-image');
    expect(String(fetchMock.mock.calls[0]?.[0])).toBe('/api/workspace/raw?path=assets%2Fdot.png&project=p_images');
    expect(new Headers(fetchMock.mock.calls[0]?.[1]?.headers).get('Authorization')).toBe('Bearer image-master-secret');
    expect(document.body.innerHTML).not.toContain('image-master-secret');

    view.unmount();
    // preact 11 runs passive-effect cleanup on unmount asynchronously, so the
    // revoke is awaited rather than assumed synchronous.
    await act(async () => { await Promise.resolve(); });
    expect(revoke).toHaveBeenCalledWith('blob:artifact-image');
    setAuthToken(null);
    store.currentProjectId.value = null;
    for (const [name, descriptor] of [
      ['createObjectURL', createDescriptor],
      ['revokeObjectURL', revokeDescriptor],
    ] as const) {
      if (descriptor) Object.defineProperty(URL, name, descriptor);
      else delete (URL as unknown as Record<string, unknown>)[name];
    }
  });
});

describe('a CSV artifact', () => {
  it('draws a table with the first row as its header', () => {
    const { container } = render(
      <Markdown text={fence('csv', 'name,qty\nbolt,4\n"nut, hex",7')} />,
    );
    const table = container.querySelector('[data-testid="artifact-table"]');
    expect(table).not.toBeNull();
    const headers = Array.from(table!.querySelectorAll('th')).map((cell) => cell.textContent);
    expect(headers).toEqual(['name', 'qty']);
    const rows = Array.from(table!.querySelectorAll('tbody tr')).map((row) =>
      Array.from(row.querySelectorAll('td')).map((cell) => cell.textContent),
    );
    expect(rows).toEqual([
      ['bolt', '4'],
      ['nut, hex', '7'],
    ]);
  });
});

describe('the card chrome', () => {
  it('copies the source, and downloads it under a safe name', async () => {
    const { container } = render(
      <Markdown text={fence('artifact html title="Signup/App form"', HTML_MOCK)} />,
    );
    const writeText = vi.spyOn(navigator.clipboard, 'writeText');
    fireEvent.click(screen.getByRole('button', { name: "Copy this artifact's source" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(HTML_MOCK + '\n'));

    const click = vi.fn();
    const original = HTMLAnchorElement.prototype.click;
    HTMLAnchorElement.prototype.click = click;
    vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: () => 'blob:x', revokeObjectURL: () => undefined }));
    fireEvent.click(container.querySelector('[aria-label^="Download this artifact"]')!);
    expect(click).toHaveBeenCalled();
    HTMLAnchorElement.prototype.click = original;
    vi.unstubAllGlobals();
  });

  it('opens full screen in a real dialog', async () => {
    render(<Markdown text={fence('artifact html title="Signup form"', HTML_MOCK)} />);
    fireEvent.click(screen.getByRole('button', { name: 'Open this artifact full screen' }));
    const dialog = screen.getByRole('dialog');
    expect(dialog.textContent).toContain('Signup form');
    await waitFor(() => expect(dialog.querySelector('iframe')).not.toBeNull());
    expect(dialog.querySelector('iframe')?.getAttribute('sandbox')).toBe('allow-scripts');
  });

  it('renders and announces a rejected copy instead of failing silently', async () => {
    // Both paths fail: the Clipboard API rejects, and the legacy textarea+execCommand
    // fallback is unavailable in jsdom, so `copyText` resolves `false`.
    vi.spyOn(navigator.clipboard, 'writeText').mockRejectedValue(new Error('denied'));
    const { container } = render(<Markdown text={fence('artifact html', HTML_MOCK)} />);

    const copy = screen.getByRole('button', { name: "Copy this artifact's source" });
    fireEvent.click(copy);

    // The rejection is visible on the control itself...
    await waitFor(() =>
      expect(container.querySelector('[data-copy-state="failed"]')?.textContent).toContain('Copy failed'),
    );
    expect(copy.getAttribute('title')).toContain('Download');
    // ...renamed for assistive technology...
    expect(copy.getAttribute('aria-label')).toContain('Copy failed');
    // ...and announced through the card's live region.
    const announcement = container.querySelector('[data-testid="artifact-announcement"]');
    expect(announcement?.getAttribute('aria-live')).toBe('polite');
    expect(announcement?.textContent).toContain('Copy failed');
    expect(announcement?.textContent).toContain('Download');
  });

  it('announces a successful copy and shows Copied on the control', async () => {
    vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(undefined);
    const { container } = render(<Markdown text={fence('artifact html', HTML_MOCK)} />);

    fireEvent.click(screen.getByRole('button', { name: "Copy this artifact's source" }));

    const announcement = container.querySelector('[data-testid="artifact-announcement"]');
    await waitFor(() => expect(announcement?.textContent).toContain('copied to the clipboard'));
    expect(container.querySelector('[data-copy-state="done"]')?.textContent).toContain('Copied');
  });

  it('marks which half is showing for assistive technology', () => {
    render(<Markdown text={fence('artifact html', HTML_MOCK)} />);
    expect(screen.getByRole('button', { name: 'Preview' }).getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByRole('button', { name: 'Source' }).getAttribute('aria-pressed')).toBe('false');
  });
});
