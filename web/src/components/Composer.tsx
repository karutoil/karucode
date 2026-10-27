import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import type { Activity } from '../state/selectors';
import { SendIcon, StopIcon, AlertIcon } from './icons';

export interface ComposerProps {
  disabledReason: string | null;
  running: boolean;
  stopping: boolean;
  status: Activity;
  onSend: (text: string) => Promise<boolean>;
  onStop: () => void;
  onOpenSettings: () => void;
  focusTick: number;
  usageLabel?: string;
}

const CHAR_COUNTER_THRESHOLD = 4000;
const MAX_TEXTAREA_HEIGHT = 236;

export function Composer({
  disabledReason,
  running,
  stopping,
  status,
  onSend,
  onStop,
  onOpenSettings,
  focusTick,
  usageLabel,
}: ComposerProps): JSX.Element {
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const textarea = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const element = textarea.current;
    if (!element) return;
    element.style.height = 'auto';
    element.style.height = `${Math.min(element.scrollHeight, MAX_TEXTAREA_HEIGHT)}px`;
  }, [draft]);

  useEffect(() => {
    if (focusTick === 0) return;
    const narrow =
      typeof window !== 'undefined' && typeof window.matchMedia === 'function'
        ? window.matchMedia('(max-width: 1023px)').matches
        : false;
    if (narrow) return;
    const timer = window.setTimeout(() => textarea.current?.focus(), 30);
    return () => window.clearTimeout(timer);
  }, [focusTick]);

  const canType = disabledReason === null;
  const canSend = canType && !running && !sending && draft.trim() !== '';

  const submit = async (): Promise<void> => {
    if (!canSend) return;
    setSending(true);
    try {
      const accepted = await onSend(draft);
      if (accepted) setDraft('');
    } finally {
      setSending(false);
    }
  };

  const placeholder = !canType
    ? disabledReason ?? 'The composer is unavailable.'
    : running
      ? 'The agent is working — draft the next message while you wait…'
      : 'Ask karucode to inspect a file, make a change, or run a command…';

  return (
    <form
      class="composer"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <div class="composer-box">
        <label class="sr-only" for="composer-input">
          Message karucode
        </label>
        <textarea
          id="composer-input"
          ref={textarea}
          class="composer-input"
          rows={2}
          value={draft}
          placeholder={placeholder}
          disabled={!canType}
          aria-describedby="composer-status"
          onInput={(event) => setDraft(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              void submit();
            } else if (event.key === 'Escape' && running) {
              event.preventDefault();
              event.stopPropagation();
              if (!stopping) onStop();
            }
          }}
        />

        <div class="composer-bottom">
          <div class="composer-status-wrap">
            <span
              id="composer-status"
              class={`composer-status status-${status.kind}`}
              role="status"
              aria-live="polite"
            >
              {disabledReason ? (
                <>
                  <AlertIcon size={13} />
                  <span>{disabledReason}</span>
                  <button type="button" class="link-button" onClick={onOpenSettings}>
                    Open settings
                  </button>
                </>
              ) : (
                <>
                  <span class={`status-dot status-${status.kind}`} aria-hidden="true" />
                  <span>{status.label}</span>
                  {usageLabel ? <span class="composer-usage">{usageLabel}</span> : null}
                </>
              )}
            </span>
            <span class="composer-hint" aria-hidden="true">
              {running ? (
                <>
                  <kbd>Esc</kbd> to stop
                </>
              ) : (
                <>
                  <kbd>Cmd</kbd>/<kbd>Ctrl</kbd> + <kbd>Enter</kbd> to send
                </>
              )}
            </span>
          </div>

          <div class="composer-actions">
            {draft.length > CHAR_COUNTER_THRESHOLD ? (
              <span class={`char-count${draft.length > 12000 ? ' is-over' : ''}`}>
                {draft.length.toLocaleString()} characters
              </span>
            ) : null}
            {running ? (
              <button
                type="button"
                class="button button-stop"
                onClick={onStop}
                disabled={stopping}
                aria-label="Stop run (Escape)"
              >
                <StopIcon size={13} />
                {stopping ? 'Stopping…' : 'Stop'}
              </button>
            ) : (
              <button
                type="submit"
                class="button button-primary button-send"
                disabled={!canSend}
                aria-label="Send message (Command or Control + Enter)"
              >
                <SendIcon size={14} />
                Send
              </button>
            )}
          </div>
        </div>
      </div>
    </form>
  );
}

