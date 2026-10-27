import type { JSX } from 'preact';
import { useMemo } from 'preact/hooks';
import { copyText } from '../lib/clipboard';
import { renderMarkdown } from '../lib/markdown';

export interface MarkdownProps {
  text: string;
  class?: string;
}

/**
 * Markdown is rendered with raw HTML disabled, sanitised by DOMPurify, and the
 * copy buttons emitted by the fence renderer are handled by delegation.
 */
export function Markdown({ text, class: className }: MarkdownProps): JSX.Element {
  const html = useMemo(() => renderMarkdown(text), [text]);

  const onClick = (event: JSX.TargetedMouseEvent<HTMLDivElement>): void => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const button = target.closest<HTMLButtonElement>('[data-copy]');
    if (!button) return;
    const block = button.closest('.code-block');
    const code = block?.querySelector('code');
    if (!code) return;
    const previous = button.textContent;
    void copyText(code.textContent ?? '').then((ok) => {
      button.textContent = ok ? 'Copied' : 'Copy failed';
      window.setTimeout(() => {
        button.textContent = previous;
      }, 1400);
    });
  };

  return (
    <div
      class={`markdown${className ? ` ${className}` : ''}`}
      onClick={onClick}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}