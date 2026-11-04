import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { Dialog } from '../Dialog';

/**
 * A destructive git action, described once so the manager can ask for it.
 *
 * Two of these exist (`discard`, branch delete / stash drop) and both are
 * irreversible from the UI, so the confirmation lives in one component instead
 * of three ad-hoc dialogs that would drift apart.
 */
export interface ConfirmRequest {
  title: string;
  description: string;
  confirmLabel: string;
  /** Extra line under the buttons: what will *not* happen, or how to undo. */
  note?: string;
  onConfirm: () => void | Promise<void>;
}

export interface GitConfirmDialogProps {
  request: ConfirmRequest | null;
  onClose: () => void;
}

export function GitConfirmDialog({ request, onClose }: GitConfirmDialogProps): JSX.Element {
  const [working, setWorking] = useState(false);
  return (
    <Dialog
      open={request !== null}
      title={request?.title ?? 'Are you sure?'}
      description={request?.description}
      onClose={onClose}
      class="gitm-confirm-dialog"
    >
      {request ? (
        <div class="gitm-confirm">
          {/* The sentence is Dialog's own `description` (announced through
              `aria-describedby`); repeating it here showed it twice. */}
          {request.note ? <p class="gitm-confirm-note">{request.note}</p> : null}
          <div class="gitm-confirm-actions">
            <button type="button" class="button" onClick={onClose}>
              Cancel
            </button>
            <button
              type="button"
              class="button button-danger"
              disabled={working}
              onClick={() => {
                setWorking(true);
                void Promise.resolve(request.onConfirm()).then(() => {
                  setWorking(false);
                  onClose();
                });
              }}
            >
              {request.confirmLabel}
            </button>
          </div>
        </div>
      ) : null}
    </Dialog>
  );
}