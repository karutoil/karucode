import type { ComponentChildren } from 'preact';
import { useId, useRef } from 'preact/hooks';
import { createPortal } from 'preact/compat';
import { useFocusTrap } from '../lib/dom';
import { CloseIcon } from './icons';

export interface DialogProps {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ComponentChildren;
  class?: string;
  showHeader?: boolean;
  labelId?: string;
  /** Optional supporting line under the title; announced as the dialog description. */
  description?: string;
}

/**
 * Accessible modal shell: focus trap, Escape to close, scroll lock, focus restore
 * and a labelled dialog landmark. Panels are mounted only while open.
 *
 * Rendered through a portal into `document.body`, never in place. `.dialog-layer`
 * is `position: fixed`, and a fixed element is only laid out against the viewport
 * while no ancestor establishes a containing block for it. The transcript
 * deliberately breaks that: `.msg` carries `content-visibility: auto` (see
 * `styles/chat.css`) so off-screen rows skip layout and paint, and paint
 * containment makes the row a containing block for fixed descendants. An image
 * lightbox opened from a tool card was therefore clipped to its own message row -
 * measured in Chromium at 691x575 inside a 1440x817 viewport - instead of covering
 * the screen. Portalling out of the transcript fixes that for every dialog, and
 * keeps working for any future ancestor that adds `filter`, `transform` or
 * `contain` for its own reasons.
 */
export function Dialog({
  open,
  title,
  onClose,
  children,
  class: className = '',
  showHeader = true,
  labelId,
  description,
}: DialogProps): ComponentChildren {
  const ref = useRef<HTMLDivElement>(null);
  const generatedId = useId();
  const titleId = labelId ?? `dialog-title-${generatedId}`;
  const descriptionId = description ? `dialog-description-${labelId ?? generatedId}` : undefined;
  useFocusTrap(ref, open, onClose);
  if (!open) return null;

  const layer = (
    <div class="dialog-layer">
      <div class="dialog-backdrop" aria-hidden="true" onClick={onClose} />
      <div
        class={`dialog ${className}`.trim()}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        ref={ref}
        data-testid="dialog"
      >
        {showHeader ? (
          <header class="dialog-header">
            <div class="dialog-headings">
              <h2 id={titleId} class="dialog-title">
                {title}
              </h2>
              {description ? (
                <p id={descriptionId} class="dialog-description">
                  {description}
                </p>
              ) : null}
            </div>
            <button type="button" class="icon-button" onClick={onClose} aria-label="Close dialog">
              <CloseIcon />
            </button>
          </header>
        ) : (
          <>
            <h2 id={titleId} class="sr-only">
              {title}
            </h2>
            {description ? (
              <p id={descriptionId} class="sr-only">
                {description}
              </p>
            ) : null}
          </>
        )}
        {children}
      </div>
    </div>
  );

  // `document` is absent in a server render and in the `node`-environment test
  // suites, where there is nothing to portal into and nothing to lay out either.
  if (typeof document === 'undefined') return layer;
  return createPortal(layer, document.body);
}