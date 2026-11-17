import type { ComponentChildren } from 'preact';
import { Overlay, type OverlayRole } from './Overlay';

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
 *
 * @deprecated Use `Overlay` with a `role`, which names the dialog's measure
 * instead of leaving each caller to author a width. This wrapper is the same
 * shell, the same portal and the same trap under the old props, kept so the call
 * sites that predate the role table keep compiling while they migrate. It does
 * not scroll its own body: callers that manage a scroller (Settings, the Theme
 * studio, the palette, the file viewer) keep doing so, which is exactly what
 * `Overlay scrollBody={false}` expresses.
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
  // The role is derived from the caller's class so the shell picks the right
  // measure for surfaces that have not migrated yet: a file or artifact is full,
  // a question or a rewind is compact, the palette has its own, and everything
  // else is the standard measure.
  const role: OverlayRole =
    className.includes('dialog-file') || className.includes('dialog-artifact')
      ? 'full'
      : className.includes('dialog-confirm') || className.includes('dialog-rewind')
        ? 'compact'
        : className.includes('dialog-palette')
          ? 'palette'
          : 'standard';
  return (
    <Overlay
      open={open}
      title={title}
      onClose={onClose}
      role={role}
      class={className}
      showHeader={showHeader}
      labelId={labelId}
      description={description}
      bodyLayout="contents"
    >
      {children}
    </Overlay>
  );
}
