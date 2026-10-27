import type { ComponentChildren } from 'preact';
import { useId, useRef } from 'preact/hooks';
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
}

/**
 * Accessible modal shell: focus trap, Escape to close, scroll lock, focus restore
 * and a labelled dialog landmark. Panels are mounted only while open.
 */
export function Dialog({
  open,
  title,
  onClose,
  children,
  class: className = '',
  showHeader = true,
  labelId,
}: DialogProps): ComponentChildren {
  const ref = useRef<HTMLDivElement>(null);
  const generatedId = useId();
  const titleId = labelId ?? `dialog-title-${generatedId}`;
  useFocusTrap(ref, open, onClose);
  if (!open) return null;

  return (
    <div class="dialog-layer">
      <div class="dialog-backdrop" aria-hidden="true" onClick={onClose} />
      <div
        class={`dialog ${className}`.trim()}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        ref={ref}
        data-testid="dialog"
      >
        {showHeader ? (
          <header class="dialog-header">
            <h2 id={titleId} class="dialog-title">
              {title}
            </h2>
            <button type="button" class="icon-button" onClick={onClose} aria-label="Close dialog">
              <CloseIcon />
            </button>
          </header>
        ) : (
          <h2 id={titleId} class="sr-only">
            {title}
          </h2>
        )}
        {children}
      </div>
    </div>
  );
}