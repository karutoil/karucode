import type { JSX } from 'preact';
import { useEffect, useId, useRef, useState } from 'preact/hooks';
import { formatDuration } from '../lib/format';
import type { CommandItem } from '../state/chat';
import { toolStatusLabel } from '../state/selectors';
import { AlertIcon, CheckIcon, ChevronDownIcon } from './icons';

export interface CommandCardProps {
  item: CommandItem;
  defaultOpen?: boolean;
}

/**
 * A shell command in the assistant column, styled like the mock's command card:
 * one summary row (outcome glyph, command, exit status, duration) that
 * discloses the output, with stdout and stderr kept apart.
 */
export function CommandCard({ item, defaultOpen = false }: CommandCardProps): JSX.Element {
  const scroller = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(defaultOpen);
  const bodyId = useId();
  const duration = formatDuration(item.durationMs);
  const outputLength = item.stdout.length + item.stderr.length;
  const hasOutput = outputLength > 0 || item.output !== '';
  const ok = item.exitCode === 0 || (item.exitCode === null && item.status === 'ok');
  const tone = ok ? 'cmd-good' : item.status === 'running' ? 'cmd-running' : item.status === 'stopped' ? 'cmd-stopped' : 'cmd-bad';

  useEffect(() => {
    if (defaultOpen) setOpen(true);
  }, [defaultOpen]);

  useEffect(() => {
    const element = scroller.current;
    if (!element || !open || item.status !== 'running') return;
    const nearBottom = element.scrollHeight - element.scrollTop - element.clientHeight < 96;
    if (nearBottom) element.scrollTop = element.scrollHeight;
  }, [outputLength, item.status, open]);

  return (
    <section
      class="command-card"
      data-status={item.status}
      data-open={open ? 'true' : 'false'}
      aria-label={`Command ${item.command}`}
    >
      <button
        type="button"
        class="command-summary"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => setOpen((previous) => !previous)}
      >
        <span class={`cmd-glyph ${tone}`} aria-hidden="true">
          {item.status === 'running' ? (
            <span class="status-dot status-running" />
          ) : ok ? (
            <CheckIcon size={14} />
          ) : item.status === 'stopped' ? (
            <span class="status-dot status-stopped" />
          ) : (
            <AlertIcon size={14} />
          )}
        </span>
        <span class="command-text" title={item.command}>
          {item.command}
        </span>
        {item.cwd && item.cwd !== '.' ? (
          <span class="command-cwd" title={item.cwd}>
            {item.cwd}
          </span>
        ) : null}
        {item.truncated ? <span class="badge badge-warn">truncated</span> : null}
        {item.exitCode !== null ? (
          <span class={`command-status ${item.exitCode === 0 ? 'is-ok' : 'is-error'}`}>
            exit {item.exitCode}
          </span>
        ) : item.status === 'running' ? (
          <span class="command-status is-running">running</span>
        ) : item.status !== 'ok' ? (
          <span class={`command-status is-${item.status}`}>{toolStatusLabel(item.status)}</span>
        ) : null}
        {duration ? <span class="command-duration">{duration}</span> : null}
        <span class="disclosure" aria-hidden="true">
          <ChevronDownIcon size={12} />
        </span>
      </button>

      <div class="cmd-output" id={bodyId} hidden={!open}>
        {open ? (
          hasOutput ? (
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
            <p class="muted small output-empty">
              {item.status === 'running' ? 'Waiting for output…' : 'No output.'}
            </p>
          )
        ) : null}
      </div>
    </section>
  );
}
