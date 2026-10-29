import type { JSX } from 'preact';
import type { Toast } from '../api/types';
import * as store from '../state/store';
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
          <button
            type="button"
            class="icon-button toast-close"
            aria-label="Dismiss notification"
            onClick={() => store.removeToast(toast.id)}
          >
            <CloseIcon size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}