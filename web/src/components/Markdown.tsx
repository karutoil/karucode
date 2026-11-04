import type { JSX } from 'preact';
import { render } from 'preact';
import { useLayoutEffect, useMemo, useRef } from 'preact/hooks';
import { copyText } from '../lib/clipboard';
import { ARTIFACT_SLOT_ATTRIBUTE, renderMarkdownDocument } from '../lib/markdown';
import { ArtifactCard } from './ArtifactCard';
import { ErrorBoundary } from './ErrorBoundary';

export interface MarkdownProps {
  text: string;
  class?: string;
  /**
   * Draw recognised fenced blocks as artifact cards. Default `true`.
   *
   * The transcript turns it off for the message that is still streaming: a fence
   * is only content once it is closed, and markdown-it closes one at the end of
   * its input, so every token of a half-written mock would otherwise paint a
   * preview, reload it and relayout the transcript.
   */
  artifacts?: boolean;
}

/**
 * Markdown is rendered with raw HTML disabled, sanitised by DOMPurify, and the
 * copy buttons emitted by the fence renderer are handled by delegation.
 *
 * Artifacts are mounted, not rendered into the tree. The reason is structural:
 * the prose and the fences arrive as one HTML string (that is what markdown-it
 * produces, and re-tokenising to interleave components would mean rendering it
 * twice), so the renderer leaves an empty placeholder element per artifact and
 * this component fills each one with `render()` once the DOM exists. Preact
 * cannot do that from the vnode tree: a `dangerouslySetInnerHTML` element has its
 * children ignored entirely, which is exactly what makes the string and the
 * component tree safe to mix this way.
 */
export function Markdown({ text, class: className, artifacts = true }: MarkdownProps): JSX.Element {
  const doc = useMemo(
    () => renderMarkdownDocument(text, { artifacts }),
    [text, artifacts],
  );
  const hostRef = useRef<HTMLDivElement>(null);
  // One object per HTML string: Preact compares `dangerouslySetInnerHTML` by
  // identity as well as by content, and a fresh object every render is a wasted
  // innerHTML rewrite - which would also unmount every artifact card.
  const markup = useMemo(() => ({ __html: doc.html }), [doc.html]);

  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host || doc.artifacts.length === 0) return;
    const mounted: HTMLElement[] = [];
    const slots = host.querySelectorAll<HTMLElement>(`[${ARTIFACT_SLOT_ATTRIBUTE}]`);
    for (const slot of Array.from(slots)) {
      const index = Number(slot.getAttribute(ARTIFACT_SLOT_ATTRIBUTE) ?? '');
      const spec = doc.artifacts[index];
      if (!spec) continue;
      // One bad artifact must not take the conversation with it: each card is its
      // own boundary, so a render that throws leaves a crash card in that slot and
      // the rest of the transcript (including the prose around it) stays put.
      render(
        <ErrorBoundary scope="this artifact">
          <ArtifactCard spec={spec} />
        </ErrorBoundary>,
        slot,
      );
      mounted.push(slot);
    }
    return () => {
      for (const slot of mounted) render(null, slot);
    };
  }, [doc]);

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
      ref={hostRef}
      onClick={onClick}
      dangerouslySetInnerHTML={markup}
    />
  );
}
