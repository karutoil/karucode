import type { ComponentChildren, JSX } from 'preact';
import { Overlay } from '../components/Overlay';
import { Button } from './Button';

export type ConfirmDialogTone = 'primary' | 'danger';

export interface ConfirmDialogProps {
  open: boolean;
  title: string;
  description?: string;
  onClose: () => void;
  onConfirm: () => void;
  confirmLabel?: string;
  cancelLabel?: string;
  tone?: ConfirmDialogTone;
  busy?: boolean;
  footerNote?: ComponentChildren;
  testId?: string;
  children?: ComponentChildren;
}

export function ConfirmDialog({
  open,
  title,
  description,
  onClose,
  onConfirm,
  confirmLabel = 'Confirm',
  cancelLabel = 'Cancel',
  tone = 'primary',
  busy = false,
  footerNote,
  testId,
  children,
}: ConfirmDialogProps): JSX.Element {
  const handleClose = (): void => {
    if (!busy) onClose();
  };

  const footer = (
    <div class="ml-auto flex items-center gap-4" data-testid={testId}>
      <Button
        variant="secondary"
        disabled={busy}
        onClick={handleClose}
        testId={testId ? `${testId}-cancel` : undefined}
      >
        {cancelLabel}
      </Button>
      <Button
        variant={tone === 'danger' ? 'danger' : 'primary'}
        loading={busy}
        onClick={() => {
          if (!busy) onConfirm();
        }}
        testId={testId ? `${testId}-confirm` : undefined}
      >
        {confirmLabel}
      </Button>
    </div>
  );

  return (
    <Overlay
      open={open}
      role="compact"
      title={title}
      description={description}
      onClose={handleClose}
      footerNote={footerNote}
      footer={footer}
      bodyLayout={children !== undefined ? 'scroll' : 'contents'}
    >
      {children !== undefined ? (
        <div class="px-8 py-5 text-body text-text-secondary">{children}</div>
      ) : null}
    </Overlay>
  );
}
