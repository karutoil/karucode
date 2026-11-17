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
  /**
   * The measure and anchor, named explicitly. Callers migrating off `Dialog`
   * should pass this; it always wins over the class fallback below.
   */
  role?: OverlayRole;
}

/**
 * The class→role table for call sites that predate the `role` prop.
 *
 * These are the exact classes the unmigrated surfaces pass, matched whole (not
 * by substring), so a caller that names a *new* class can never accidentally
 * land on a measure by containing a word like `dialog-confirm` in something
 * else. Anything not listed here is the standard measure.
 */
const CLASS_ROLES: Readonly<Record<string, OverlayRole>> = {
  'dialog-file': 'full',
  'dialog-artifact': 'full',
  'dialog-confirm': 'compact',
  'dialog-rewind': 'compact',
  'dialog-palette': 'palette',
};

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
 * `Overlay bodyLayout="contents"` expresses — the children are laid out as direct
 * children of the surface, with no `.overlay-body` scroller wrapped around them.
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
  role: explicitRole,
}: DialogProps): ComponentChildren {
  // The role comes from the caller when it names one. Only the surfaces that
  // have not migrated yet fall back to `CLASS_ROLES`, which matches the whole
  // class: a new class can never land on a measure by containing a word.
  const role: OverlayRole = explicitRole ?? CLASS_ROLES[className.trim()] ?? 'standard';
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
