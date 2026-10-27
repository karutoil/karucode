import { describe, expect, it } from 'vitest';
import { render } from '@testing-library/preact';
import { Markdown } from '../components/Markdown';

describe('Markdown', () => {
  it('does not execute raw HTML from model output', () => {
    const { container } = render(
      <Markdown
        text={
          'Before\n\n<script>window.__karucode_pwned = 1;</script>\n\n<img src="x" onerror="window.__karucode_pwned = 2">\n\nAfter'
        }
      />,
    );

    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('img')).toBeNull();
    expect((window as unknown as { __karucode_pwned?: number }).__karucode_pwned).toBeUndefined();
    expect(container.textContent).toContain('<script>');
    expect(container.textContent).toContain('After');
  });

  it('sanitises javascript: links', () => {
    const { container } = render(<Markdown text={'[click me](javascript:alert(1))'} />);
    const anchors = Array.from(container.querySelectorAll('a'));
    for (const anchor of anchors) {
      expect(anchor.getAttribute('href') ?? '').not.toContain('javascript:');
    }
    expect(container.textContent).toContain('click me');
  });

  it('adds noopener rel and target to external links', () => {
    const { container } = render(<Markdown text={'[docs](https://example.com/page)'} />);
    const anchor = container.querySelector('a');
    expect(anchor?.getAttribute('target')).toBe('_blank');
    expect(anchor?.getAttribute('rel')).toBe('noopener noreferrer');
  });

  it('renders fenced code with a language label, highlight classes and a copy button', () => {
    const { container } = render(
      <Markdown text={'```rust\nfn main() { println!("hi"); }\n```'} />,
    );
    expect(container.querySelector('.code-block-lang')?.textContent).toBe('rust');
    expect(container.querySelector('.code-block-pre code')?.textContent).toContain('fn main()');
    expect(container.querySelector('.hljs-keyword')?.textContent).toBe('fn');
    const copy = container.querySelector<HTMLButtonElement>('button[data-copy]');
    expect(copy).not.toBeNull();
    expect(copy?.getAttribute('aria-label')).toContain('Copy rust code');
  });

  it('escapes code for unknown languages', () => {
    const { container } = render(<Markdown text={'```not-a-language\n<script>x</script>\n```'} />);
    expect(container.querySelector('.code-block-lang')?.textContent).toBe('not-a-language');
    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('code')?.textContent).toContain('<script>x</script>');
  });
});