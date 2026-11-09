import { describe, expect, it } from 'vitest';
import { render } from '@testing-library/preact';
import { Markdown } from './Markdown';

/** One settled paragraph: a word-run and the blank line that ends it. */
function block(index: number): string {
  return `Paragraph ${index}. ${'word '.repeat(40)}\n\n`;
}

/** Enough settled blocks for the boundary to have moved. */
function settled(): string {
  return Array.from({ length: 12 }, (_, i) => block(i + 1)).join('');
}

describe('Markdown while a message is streaming', () => {
  it('renders the settled blocks as Markdown and the newest block as Markdown too', () => {
    render(<Markdown text={`${settled()}**still writing**`} streaming />);

    const stable = document.querySelector('.markdown-settled');
    expect(stable).not.toBeNull();
    expect(stable?.querySelectorAll('p').length).toBeGreaterThan(0);

    // The tail keeps its formatting: what the reader sees mid-answer is what the
    // settled message will show.
    const live = document.querySelector('.markdown-live');
    expect(live?.querySelector('strong')?.textContent).toBe('still writing');
    expect(live?.classList.contains('markdown-live-raw')).toBe(false);
  });

  it('escapes the live tail instead of rendering its HTML', () => {
    render(<Markdown text={`${settled()}<img src=x onerror="alert(1)">`} streaming />);

    expect(document.querySelector('.markdown-live img')).toBeNull();
    expect(document.querySelector('.markdown-live')?.textContent).toContain('<img src=x');
  });

  it('draws an unterminated fence as text on the monospace surface, without highlighting it', () => {
    render(<Markdown text={`${settled()}\`\`\`ts\nconst a = 1;\n`} streaming />);

    const live = document.querySelector('.markdown-live');
    expect(live?.classList.contains('markdown-live-code')).toBe(true);
    expect(live?.classList.contains('markdown-live-raw')).toBe(true);
    expect(live?.querySelector('.hljs-string, .hljs-keyword')).toBeNull();
  });

  it('highlights a fence that has already closed, because that part is settled', () => {
    const fence = `\`\`\`rust\nfn one() {}\n\`\`\`\n\n`;
    render(<Markdown text={`${settled()}${fence}tail`} streaming />);

    expect(document.querySelector('.markdown-settled .code-block')).not.toBeNull();
  });

  it('falls back to plain text for a tail too big to re-parse per token', () => {
    const huge = `- an item in a very long list ${'x '.repeat(3_000)}`;
    render(<Markdown text={`${settled()}${huge}`} streaming />);

    const live = document.querySelector('.markdown-live');
    expect(live?.classList.contains('markdown-live-raw')).toBe(true);
    expect(live?.textContent).toBe(huge);
  });

  it('renders no artifact cards while the message is still arriving', () => {
    const fence = '```artifact html title="x"\n<p>hi</p>\n```\n\n';
    render(<Markdown text={`${settled()}${fence}tail`} streaming artifacts />);

    expect(document.querySelector('.artifact-slot')).toBeNull();
  });

  it('renders the whole message as Markdown once it settles', () => {
    const text = `${settled()}**done**`;
    const { rerender } = render(<Markdown text={text} streaming />);
    expect(document.querySelector('.markdown-live strong')?.textContent).toBe('done');

    rerender(<Markdown text={text} />);

    expect(document.querySelector('.markdown-live')).toBeNull();
    expect(document.querySelector('.markdown-settled')).toBeNull();
    expect(document.querySelector('.markdown')?.innerHTML).toContain('<strong>done</strong>');
  });

  it('keeps the settled prefix when only the tail grows', () => {
    const { rerender } = render(<Markdown text={`${settled()}one`} streaming />);
    const first = document.querySelector('.markdown-settled')?.innerHTML;

    rerender(<Markdown text={`${settled()}one two`} streaming />);

    // The prefix was not re-parsed into a different DOM: same markup, and the
    // tail simply grew.
    expect(document.querySelector('.markdown-settled')?.innerHTML).toBe(first);
    expect(document.querySelector('.markdown-live')?.textContent?.trim()).toBe('one two');
  });

  it('resets to plain text when the answer is replaced in place', () => {
    const { rerender } = render(<Markdown text={settled()} streaming />);
    expect(document.querySelector('.markdown-settled')).not.toBeNull();

    // `text_replaced` can rewrite the whole live answer at once.
    const replaced = 'Replaced answer. '.repeat(300);
    rerender(<Markdown text={replaced} streaming />);

    // Over the tail's size cap, so it is shown as the text it is - and the stale
    // settled prefix is gone rather than describing text that no longer exists.
    const live = document.querySelector('.markdown-live');
    expect(document.querySelector('.markdown-settled')).toBeNull();
    expect(live?.textContent).toBe(replaced);
    expect(live?.classList.contains('markdown-live-raw')).toBe(true);
  });

  it('shows nothing until the first delta arrives', () => {
    render(<Markdown text="" streaming />);
    expect(document.querySelector('.markdown-live')).toBeNull();
    expect(document.querySelector('.markdown-settled')).toBeNull();
  });
});
