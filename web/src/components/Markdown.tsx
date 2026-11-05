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
  // The copy buttons live inside the sanitised HTML string, so their own text is
  // the only thing this component can change about them. A live region beside the
  // prose is what a screen reader hears: the button keeps its aria-label, and
  // swapping its visible text alone would announce nothing.
  const statusRef = useRef<HTMLParagraphElement>(null);
  const announce = (message: string): void => {
    if (statusRef.current) statusRef.current.textContent = message;
  };
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
    const label = button.getAttribute('aria-label') ?? 'Code';
    void copyText(code.textContent ?? '').then((ok) => {
      button.textContent = ok ? 'Copied' : 'Copy failed';
      button.dataset.copyState = ok ? 'done' : 'failed';
      announce(
        ok
          ? `${label.replace(/^Copy /, '')} copied to the clipboard.`
          : 'Copy failed: the clipboard is unavailable here. Select the code and copy it manually.',
      );
      window.setTimeout(() => {
        button.textContent = previous;
        delete button.dataset.copyState;
        announce('');
      }, 1400);
    });
  };

  return (
    <>
      <div
        class={`markdown${className ? ` ${className}` : ''}`}
        ref={hostRef}
        onClick={onClick}
        dangerouslySetInnerHTML={markup}
      />
      {/* Always mounted, so the announcement is a change to a live region rather
          than a region that appears with its text (which some readers miss).
          `aria-live` rather than `role="status"`: the transcript already has
          status regions for notices, and a second landmark-role would make every
          "the status" query ambiguous. */}
      <p
        class="sr-only"
        aria-live="polite"
        aria-atomic="true"
        ref={statusRef}
        data-testid="markdown-copy-status"
      />
    </>
  );
}
