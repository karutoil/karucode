import type { JSX } from 'preact';
import type { ErrorItem, InterruptedItem, NoticeItem } from '../../state/chat';

export function InterruptedNotice({ item }: { item: InterruptedItem }): JSX.Element {
  return (
    <div class="notice notice-interrupted" role="status">
      <strong>Interrupted.</strong> {item.message} Send a new message to continue.
    </div>
  );
}

export function RunErrorNotice({ item }: { item: ErrorItem }): JSX.Element {
  return (
    <div class="run-error" role="alert">
      <p class="run-error-title">Error</p>
      <p class="run-error-message">{item.message}</p>
    </div>
  );
}

export function SystemNotice({ item }: { item: NoticeItem }): JSX.Element {
  return (
    <p class={`notice notice-${item.level}`} role={item.level === 'error' ? 'alert' : 'status'}>
      {item.message}
    </p>
  );
}
