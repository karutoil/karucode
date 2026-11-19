import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { ConfirmDialog, Text } from '../../ui';

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

/**
 * The shell is `ConfirmDialog` (compact measure, the same question-with-buttons
 * surface every other confirmation in the product uses). What stays here is the
 * git-specific part: an action that can be asynchronous, so the dialog reports
 * itself busy - the confirm button spins and is disabled, and the dialog cannot
 * be dismissed out from under a delete that is already on its way to the server.
 */
export function GitConfirmDialog({ request, onClose }: GitConfirmDialogProps): JSX.Element {
  const [working, setWorking] = useState(false);

  const run = (): void => {
    if (request === null) return;
    setWorking(true);
    void Promise.resolve(request.onConfirm()).then(() => {
      setWorking(false);
      onClose();
    });
  };

  return (
    <ConfirmDialog
      open={request !== null}
      title={request?.title ?? 'Are you sure?'}
      description={request?.description}
      confirmLabel={request?.confirmLabel ?? 'Confirm'}
      tone="danger"
      busy={working}
      onClose={onClose}
      onConfirm={run}
      testId="git-confirm"
      // The note is a footer line, which is where `Overlay` expects a surface to
      // explain what it refuses or how to undo it - the description above is the
      // sentence assistive tech announces as the dialog's own.
      footerNote={
        request?.note !== undefined ? (
          <Text as="p" size="tiny" tone="muted" class="gitm-confirm-note">
            {request.note}
          </Text>
        ) : undefined
      }
    />
  );
}
