import type { JSX } from 'preact';
import { render } from 'preact';
import { useLayoutEffect, useMemo, useRef } from 'preact/hooks';
import type { TargetedMouseEvent } from 'preact';
import { copyText } from '../lib/clipboard';
import { ARTIFACT_SLOT_ATTRIBUTE, renderMarkdownDocument } from '../lib/markdown';
import { useResolvedMarkdownImages } from '../lib/markdownImages';
import {
  advanceSplit,
  initialSplit,
  splitStreamingText,
  startsFence,
  type StreamingSplit,
} from '../lib/streamingMarkdown';
import * as store from '../state/store';
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
   * preview, reload it and relayout the transcript. `streaming` forces it off
   * for the same reason (see below).
   */
  artifacts?: boolean;
  /**
   * This text is still arriving, one delta at a time.
   *
   * Parsing the whole message on every delta is quadratic in the answer's
   * length: measured in the shipped bundle, a 120 KiB answer cost ~50 ms per
   * render, so a 40-delta/s stream left the tab at 14 fps and every click - a
   * project switch above all - queued behind a render. With this flag the
   * message is drawn in two halves (`lib/streamingMarkdown.ts`): the blocks that
   * are finished, rendered as Markdown and left alone until the boundary moves,
   * and the block still being written, rendered as plain text. The cost of one
   * delta becomes the size of that tail instead of the size of the answer.
   *
   * The finished message renders once, fully, the moment `streaming` goes back
   * to false - so nothing about the settled transcript changes.
   */
  streaming?: boolean;
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
 *
 * The two halves are separate components rather than branches inside one, so a
 * message that starts streaming does not hand its hooks a different meaning
 * mid-flight.
 */
export function Markdown(props: MarkdownProps): JSX.Element {
  return props.streaming ? <StreamingMarkdown {...props} /> : <SettledMarkdown {...props} />;
}

/**
 * The copy buttons live inside the sanitised HTML string, so their own text is
 * the only thing a component can change about them. A live region beside the
 * prose is what a screen reader hears: the button keeps its aria-label, and
 * swapping its visible text alone would announce nothing.
 */
function useCopyStatus(): {
  statusRef: { current: HTMLParagraphElement | null };
  announce: (message: string) => void;
} {
  const statusRef = useRef<HTMLParagraphElement>(null);
  const announce = (message: string): void => {
    if (statusRef.current) statusRef.current.textContent = message;
  };
  return { statusRef, announce };
}

/** One `data-copy` button, delegated from whichever element contains the prose. */
function onClickCopy(
  event: TargetedMouseEvent<HTMLDivElement>,
  announce: (message: string) => void,
): void {
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
        : `Copy failed: the clipboard is unavailable here. Select the code and copy it manually.`,
    );
    window.setTimeout(() => {
      button.textContent = previous;
      delete button.dataset.copyState;
      announce('');
    }, 1400);
  });
}

/** A message that has stopped changing: one parse, one `innerHTML` swap. */
function SettledMarkdown({ text, class: className, artifacts = true }: MarkdownProps): JSX.Element {
  const doc = useMemo(
    () => renderMarkdownDocument(text, { artifacts }),
    [text, artifacts],
  );
  const hostRef = useRef<HTMLDivElement>(null);
  const { statusRef, announce } = useCopyStatus();
  const projectId = store.currentProjectId.value;
  // Images are the one part of a rendered message that is not already in the
  // HTML: a workspace file has to be fetched with the session's credentials, and
  // a remote one can never load under this app's policy (see
  // `lib/markdownImages.ts`). The effect rewrites this DOM, so it runs whenever
  // the rendered HTML is replaced.
  useResolvedMarkdownImages(hostRef, doc.html, projectId, true);
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

  return (
    <>
      <div
        class={`markdown${className ? ` ${className}` : ''}`}
        ref={hostRef}
        onClick={(event) => onClickCopy(event, announce)}
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

/**
 * A message that is still arriving.
 *
 * The settled prefix is rendered exactly like a finished message, and only when
 * the boundary moves - which `lib/streamingMarkdown.ts` keeps rare and
 * proportional, so the total parsing done over one answer is linear in its
 * length instead of quadratic. The tail is one block: what a delta costs is the
 * length of the block being written, not the length of the answer.
 *
 * The split state lives in a ref because it is a cache of what has already been
 * scanned, not state the UI reads. It is written from `useMemo` - the render
 * pass is synchronous and single-threaded, and this is the only writer - so a
 * second delta in the same tick advances the same state rather than rescanning.
 */
function StreamingMarkdown({ text, class: className }: MarkdownProps): JSX.Element {
  const splitRef = useRef<StreamingSplit>(initialSplit());
  const split = useMemo(() => {
    const next = advanceSplit(splitRef.current, text);
    splitRef.current = next;
    return next;
  }, [text]);
  const { stable, live } = useMemo(() => splitStreamingText(text, split), [text, split]);
  // Artifacts stay off while streaming: a half-written fence is not content
  // yet, and a card that appears and is replaced on the next delta is worse
  // than one that appears when the fence closes. Highlighting stays on: the
  // prefix is settled text the reader is reading, and the boundary only moves a
  // handful of times, so the colour is what they will see when it ends.
  const doc = useMemo(() => renderMarkdownDocument(stable, { artifacts: false }), [stable]);
  const markup = useMemo(() => ({ __html: doc.html }), [doc.html]);
  const { statusRef, announce } = useCopyStatus();

  return (
    <>
      <div
        class={`markdown markdown-streaming${className ? ` ${className}` : ''}`}
        onClick={(event) => onClickCopy(event, announce)}
        data-streaming="true"
      >
        {stable ? <div class="markdown-settled" dangerouslySetInnerHTML={markup} /> : null}
        {live ? <StreamingTail text={live} inFence={split.inFence} /> : null}
      </div>
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

/**
 * How much of the block still being written is rendered as Markdown.
 *
 * The tail is re-rendered on every delta, so it has to stay cheap: below this
 * size a parse costs a couple of milliseconds and the reader keeps headings,
 * lists and emphasis while the answer is arriving. Past it - a very long list, a
 * single enormous paragraph - the tail falls back to plain text, which is the
 * one place the live view is coarser than the finished message.
 */
const TAIL_MARKDOWN_MAX_CHARS = 4_096;

/**
 * The block still being written.
 *
 * It is rendered as Markdown (with highlighting off) while it is small, so what
 * the reader sees mid-answer is what they will see after it settles, and as text
 * when it is too big to re-parse per token. An unterminated fence always takes
 * the text path on the monospace surface it is about to become: re-highlighting
 * half-written code on every delta is the cost this whole split exists to avoid,
 * and the boundary never falls inside a fence, so an open fence always begins
 * the tail (the `startsFence` check covers a message with no boundary yet).
 */
function StreamingTail({ text, inFence }: { text: string; inFence: boolean }): JSX.Element {
  const fence = inFence && startsFence(text);
  const doc = useMemo(
    () =>
      fence || text.length > TAIL_MARKDOWN_MAX_CHARS
        ? null
        : renderMarkdownDocument(text, { artifacts: false, highlight: false }),
    [text, fence],
  );
  const markup = useMemo(() => (doc ? { __html: doc.html } : null), [doc]);
  if (!markup) {
    const classes = ['markdown-live', 'markdown-live-raw'];
    if (fence) classes.push('markdown-live-code');
    return <div class={classes.join(' ')}>{text}</div>;
  }
  return <div class="markdown-live" dangerouslySetInnerHTML={markup} />;
}
