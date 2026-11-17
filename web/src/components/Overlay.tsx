import type { ComponentChildren } from 'preact';
import { useId, useRef } from 'preact/hooks';
import { createPortal } from 'preact/compat';
import { useFocusTrap } from '../lib/dom';
import { CloseIcon } from './icons';

/**
 * The measure and anchor of an overlay: a named role, never a width.
 *
 * Before this, every dialog re-authored its own
 * `width: min(Npx, calc(100vw - Mpx))` and its own inset — 460, 520, 560, 620,
 * 760, 960, 1040 and 1200px with gaps of 24, 28, 32 and 48px. Two dialogs that
 * were the same *kind* of surface (Settings and the Theme studio: both tall
 * configuration sheets) therefore had nothing in common, and a third size
 * appeared every time someone needed one. The table below is the whole
 * vocabulary; `styles/overlays.css` owns the geometry for each name.
 *
 * - `compact` — a question with buttons: confirmations, rewind. 480px, top-anchored.
 * - `standard` — a form with fields: project, branch, model edit, a skill. 620px.
 * - `wide` — a list to read or pick from: the catalog, the notification centre. 860px.
 * - `tall` — a two-pane configuration document: Settings. 960px, capped height.
 * - `drawer` — the same document, slid in from the right: the Theme studio. 620px.
 * - `full` — an object to look at: a file, an artifact, an image. Up to 1200px, 92dvh.
 * - `palette` — a search field over a list: the command palette. 560px, top-anchored.
 */
export type OverlayRole = 'compact' | 'standard' | 'wide' | 'tall' | 'drawer' | 'full' | 'palette';

/** How the surface's body participates in its flex column (see `bodyLayout`). */
export type OverlayBodyLayout = 'scroll' | 'static' | 'contents';

export interface OverlayProps {
  open: boolean;
  /** The dialog's accessible name, rendered as its `<h2>`. */
  title: string;
  onClose: () => void;
  children: ComponentChildren;
  /** The measure and anchor. A role, never a width (see `OverlayRole`). */
  role?: OverlayRole;
  /** Extra classes for the surface itself, when a caller really owns a rule. */
  class?: string;
  /** `false` draws the heading for assistive tech only (the body draws its own chrome). */
  showHeader?: boolean;
  labelId?: string;
  /** One supporting line under the title; announced as the dialog description. */
  description?: string;
  /**
   * A note pinned to the left of the footer, beside the actions: what the
   * surface refuses, what a choice commits, how to undo it. It is the one place
   * a dialog is allowed to explain itself, which is why the heading no longer
   * grows a second one.
   */
  footerNote?: ComponentChildren;
  /**
   * The footer itself. Omit it for a surface that has no decision to make (the
   * file viewer, the palette, an image): a footer band with no buttons is a
   * third line of chrome over a document that already fills the dialog.
   */
  footer?: ComponentChildren;
  /**
   * How the body participates in the document's flex column.
   *
   * - `scroll` (default) — one `.overlay-body` that owns the scroll.
   * - `static` — the body does not scroll; a surface whose own panes do
   *   (Settings: rail + panel) brings its own scroller, because a nested pair of
   *   scrollers traps the wheel in the wrong one.
   * - `contents` — no wrapper at all: the children are laid out as direct
   *   children of the surface, which is what every pre-`Overlay` call site
   *   already assumed about its own chrome.
   */
  bodyLayout?: OverlayBodyLayout;
}

/**
 * The one modal shell.
 *
 * Every dialog in the product is the same three-band document:
 *
 *   ┌──────────────────────────────┐
 *   │ title                        │  `.overlay-header` — name, one line, close
 *   ├──────────────────────────────┤  hairline (`--line`)
 *   │ body — the only scroller     │  `.overlay-body`
 *   ├──────────────────────────────┤  hairline
 *   │ note            cancel  act  │  `.overlay-footer`
 *   └──────────────────────────────┘
 *
 * What varies is the measure, and that is a word (`role`), not a number.
 *
 * Behaviour, implemented once: portal to `document.body` (a fixed layer inside
 * the transcript is clipped by `content-visibility: auto` — see `Dialog`), a
 * focus trap with Escape and focus restore (`useFocusTrap`), one body scroll
 * lock, `role="dialog"` + `aria-modal="true"` + a labelled name, a backdrop that
 * closes, and one scrim for the whole family (`.dialog-backdrop`, thinned in
 * light mode and owned by `overlays.css`).
 *
 * `Dialog.tsx` is the same contract under the old name and props, so the call
 * sites that predate this file keep working while they migrate onto `role`.
 */
export function Overlay({
  open,
  title,
  onClose,
  children,
  role = 'standard',
  class: className = '',
  showHeader = true,
  labelId,
  description,
  footerNote,
  footer,
  bodyLayout = 'scroll',
}: OverlayProps): ComponentChildren {
  const ref = useRef<HTMLDivElement>(null);
  const generatedId = useId();
  const titleId = labelId ?? `overlay-title-${generatedId}`;
  const descriptionId = description ? `overlay-description-${labelId ?? generatedId}` : undefined;
  useFocusTrap(ref, open, onClose);
  if (!open) return null;

  // No longer emitted: it was a hook no stylesheet ever defined, so every
  // `showHeader={false}` surface (the palette) carried a dead class. The flag's
  // real effect is the `sr-only` title below, which is what the shell keys on.
  const surfaceClass = ['overlay', `overlay-${role}`, className].filter(Boolean).join(' ');

  const layer = (
    <div class="dialog-layer" data-overlay-role={role}>
      <div class="dialog-backdrop" aria-hidden="true" onClick={onClose} />
      <div
        class={surfaceClass}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        ref={ref}
        data-testid="dialog"
        data-role={role}
      >
        {showHeader ? (
          <header class="overlay-header">
            <div class="overlay-headings">
              <h2 id={titleId} class="overlay-title">
                {title}
              </h2>
              {description ? (
                <p id={descriptionId} class="overlay-description">
                  {description}
                </p>
              ) : null}
            </div>
            <button
              type="button"
              class="overlay-close"
              onClick={onClose}
              aria-label="Close dialog"
            >
              <CloseIcon size={14} />
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
        {bodyLayout === 'contents' ? (
          children
        ) : (
          <div class={`overlay-body${bodyLayout === 'static' ? ' is-static' : ''}`}>
            {children}
          </div>
        )}
        {footer !== undefined ? (
          // The note travels *inside* the actions slot, not beside it: a
          // surface that brings its own action row (Settings' error summary,
          // the Theme studio's autosave strip) already lays out "note, then
          // buttons", and two siblings in a wrapping row would re-wrap on a
          // narrow viewport in the wrong order.
          <footer class="overlay-footer">
            <div class="overlay-footer-actions">
              {footerNote !== undefined ? (
                <div class="overlay-footer-note">{footerNote}</div>
              ) : null}
              {footer}
            </div>
          </footer>
        ) : null}
      </div>
    </div>
  );

  // `document` is absent in a server render and in the `node`-environment test
  // suites, where there is nothing to portal into and nothing to lay out either.
  if (typeof document === 'undefined') return layer;
  return createPortal(layer, document.body);
}
