import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { formatDuration } from '../lib/format';
import type { CommandItem } from '../state/chat';
import { ChevronRightIcon } from './icons';

export interface CommandCardProps {
  item: CommandItem;
  defaultOpen?: boolean;
}

export function CommandCard({ item, defaultOpen = false }: CommandCardProps): JSX.Element {
  const scroller = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(defaultOpen);
  const duration = formatDuration(item.durationMs);
  const outputLength = item.stdout.length + item.stderr.length;
  const hasOutput = outputLength > 0 || item.output !== '';

  useEffect(() => {
    if (defaultOpen) setOpen(true);
  }, [defaultOpen]);

  useEffect(() => {
    const element = scroller.current;
    if (!element || item.status !== 'running') return;
    const nearBottom = element.scrollHeight - element.scrollTop - element.clientHeight < 96;
    if (nearBottom) element.scrollTop = element.scrollHeight;
  }, [outputLength, item.status]);

  return (
    <details
      class="command-card"
      data-status={item.status}
      open={open}
      onToggle={(event) => {
        const next = event.currentTarget.open;
        if (next !== open) setOpen(next);
      }}
    >
      <summary class="command-row">
        <span class={`status-dot status-${item.status}`} aria-hidden="true" />
        <span class="command-prompt" aria-hidden="true">
          $
        </span>
        <span class="command-text" title={item.command}>
          {item.command}
        </span>
        {item.cwd && item.cwd !== '.' ? <span class="command-cwd">{item.cwd}</span> : null}
        {item.truncated ? <span class="badge badge-warn">truncated</span> : null}
        {item.exitCode !== null ? (
          <span class={`badge ${item.exitCode === 0 ? 'badge-ok' : 'badge-danger'}`}>exit {item.exitCode}</span>
        ) : null}
        {duration ? <span class="command-duration">{duration}</span> : null}
        <span class="disclosure" aria-hidden="true">
          <ChevronRightIcon size={13} />
        </span>
      </summary>
      <div class="command-body">
        {hasOutput ? (
          <div class="command-scroll" ref={scroller}>
            {item.stdout !== '' ? (
              <section class="command-stream" aria-label="stdout">
                <span class="stream-label">stdout</span>
                <pre class="output-pre">{item.stdout}</pre>
              </section>
            ) : null}
            {item.stderr !== '' ? (
              <section class="command-stream" aria-label="stderr">
                <span class="stream-label stream-label-error">stderr</span>
                <pre class="output-pre output-pre-error">{item.stderr}</pre>
              </section>
            ) : null}
            {item.stdout === '' && item.stderr === '' && item.output !== '' ? (
              <section class="command-stream" aria-label="output">
                <pre class="output-pre">{item.output}</pre>
              </section>
            ) : null}
          </div>
        ) : (
          <p class="muted small">{item.status === 'running' ? 'Waiting for output…' : 'No output.'}</p>
        )}
      </div>
    </details>
  );
}