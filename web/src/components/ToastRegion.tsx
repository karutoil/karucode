import type { JSX } from 'preact';
import type { Toast } from '../api/types';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Button, IconButton } from '../ui';
import { AlertIcon, CheckIcon, CloseIcon, SparkIcon } from './icons';

function ToastIcon({ tone }: { tone: Toast['tone'] }): JSX.Element {
  if (tone === 'error') return <AlertIcon size={16} />;
  if (tone === 'success') return <CheckIcon size={16} />;
  return <SparkIcon size={16} />;
}

export function ToastRegion(): JSX.Element {
  const toasts = store.toasts.value;
  return (
    <div class="toast-region" aria-label="Notifications">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          class={`toast toast-${toast.tone}`}
          role={toast.tone === 'error' ? 'alert' : 'status'}
        >
          <span class="toast-icon" aria-hidden="true">
            <ToastIcon tone={toast.tone} />
          </span>
          <span class="toast-message">{toast.message}</span>
          {/* The toast's two controls are the shared actions. `.toast-action`
              and `.toast-close` stay on them as the sheet's own hooks (the row's
              flex/order and the close button's negative margin are still owned
              there); the ink, size and hover fill are the primitive's now. */}
          {toast.sessionId ? (
            <Button
              variant="secondary"
              size="sm"
              class="toast-action"
              testId={`toast-open-${toast.id}`}
              onClick={() => {
                actions.focusSession(toast.sessionId as string);
                store.removeToast(toast.id);
              }}
            >
              Open
            </Button>
          ) : null}
          <IconButton
            size="sm"
            class="toast-close"
            label="Dismiss notification"
            icon={<CloseIcon size={14} />}
            testId={`toast-dismiss-${toast.id}`}
            onClick={() => store.removeToast(toast.id)}
          />
        </div>
      ))}
    </div>
  );
}