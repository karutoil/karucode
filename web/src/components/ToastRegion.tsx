import type { JSX } from 'preact';
import * as store from '../state/store';
import { CloseIcon } from './icons';

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