import type { JSX } from 'preact';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { formatDuration } from '../lib/format';
import type { CommandItem } from '../state/chat';
import { toolStatusLabel } from '../state/selectors';
import { AlertIcon, CheckIcon, ChevronDownIcon } from './icons';
import { TpsBadge } from './TpsBadge';

export interface CommandCardProps {
  item: CommandItem;
  defaultOpen?: boolean;
  /**
   * `card` is the mock's command card: a header band, the output well beneath.
   * `row` is the same command as one line in the transcript's "Rows" style, with
   * the same output opening beneath it. The transcript picks it from the theme
   * (`ThemeToolStyle`), so a run of shell commands reads as a list rather than a
   * stack of boxes.
   *
   * Both variants keep the two things a reader acts on: the outcome (exit code
   * or running/stopped label) is always in the header line, and stdout/stderr
   * are one disclosure away - never dropped for being "compact".
   */
  variant?: 'card' | 'row';
}

/** The outcome glyph: a running dot, a tick, a stop dot, or a failure alert. */
function CommandGlyph({ item, ok }: { item: CommandItem; ok: boolean }): JSX.Element {
  if (item.status === 'running') return <span class="status-dot status-running" />;
  if (ok) return <CheckIcon size={14} />;
  if (item.status === 'stopped') return <span class="status-dot status-stopped" />;
  return <AlertIcon size={14} />;
}

/** The exit code, or the status when there is no exit code to show. */
function CommandOutcome({ item }: { item: CommandItem }): JSX.Element | null {
  if (item.exitCode !== null) {
    return (
      <span class={`command-status ${item.exitCode === 0 ? 'is-ok' : 'is-error'}`}>
        exit {item.exitCode}
      </span>
    );
  }
  if (item.status === 'running') return <span class="command-status is-running">running</span>;
  if (item.status !== 'ok') {
    return <span class={`command-status is-${item.status}`}>{toolStatusLabel(item.status)}</span>;
  }
  return null;
}

/**
 * The output well shared by both variants: stdout and stderr kept apart, with
 * the restored-history fallback, the "waiting" state and the "no output" state
 * spelled out. Only mounted while the disclosure is open, so a collapsed row
 * costs nothing.
 */
function CommandOutput({ item }: { item: CommandItem }): JSX.Element {
  const scroller = useRef<HTMLDivElement>(null);
  const outputLength = item.stdout.length + item.stderr.length;
  const hasOutput = outputLength > 0 || item.output !== '';

  // Follow reader intent, not the newly measured DOM: after a huge burst its
  // scrollHeight can make a previously pinned reader look far from the bottom.
  const following = useRef(true);
  const previousTop = useRef(0);

  useLayoutEffect(() => {
    const element = scroller.current;
    if (!element || item.status !== 'running' || !following.current) return;
    element.scrollTop = element.scrollHeight;
    previousTop.current = element.scrollTop;
  }, [outputLength, item.status]);

  const onScroll = (element: HTMLDivElement): void => {
    const top = element.scrollTop;
    const nearBottom = element.scrollHeight - top - element.clientHeight < 96;
    // Any upward scroll is an explicit request to read history, even when it is
    // less than the usual 96px near-bottom tolerance.
    following.current = top >= previousTop.current && nearBottom;
    previousTop.current = top;
  };

  if (!hasOutput) {
    return (
      <p class="muted small output-empty">
        {item.status === 'running' ? 'Waiting for output…' : 'No output.'}
      </p>
    );
  }
  return (
    <div class="command-scroll" ref={scroller} onScroll={(event) => onScroll(event.currentTarget)}>
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
  );
}

/**
 * A shell command in the assistant column, styled like the mock's command card:
 * one summary row (outcome glyph, command, exit status, duration) that
 * discloses the output, with stdout and stderr kept apart.
 */
export function CommandCard({
  item,
  defaultOpen = false,
  variant = 'card',
}: CommandCardProps): JSX.Element {
  const [open, setOpen] = useState(defaultOpen);
  const bodyId = useId();
  const duration = formatDuration(item.durationMs);
  const ok = item.exitCode === 0 || (item.exitCode === null && item.status === 'ok');
  const tone = ok ? 'cmd-good' : item.status === 'running' ? 'cmd-running' : item.status === 'stopped' ? 'cmd-stopped' : 'cmd-bad';

  // A running command opens itself when it arrives running (the reader is
  // watching it work), whether this card was mounted already or the call turned
  // running after mount.
  useEffect(() => {
    if (defaultOpen) setOpen(true);
  }, [defaultOpen]);

  const toggle = (): void => setOpen((previous) => !previous);
  const cwd = item.cwd && item.cwd !== '.' ? item.cwd : null;

  if (variant === 'row') {
    return (
      <section
        class="command-row"
        data-status={item.status}
        data-open={open ? 'true' : 'false'}
        aria-label={`Command ${item.command}`}
      >
        <button
          type="button"
          class="command-row-head"
          aria-expanded={open}
          aria-controls={bodyId}
          title={item.command}
          onClick={toggle}
        >
          <span class={`cmd-glyph ${tone}`} aria-hidden="true">
            <CommandGlyph item={item} ok={ok} />
          </span>
          <span class="command-text" title={item.command}>
            {item.command}
          </span>
          {cwd ? (
            <span class="command-cwd" title={cwd}>
              {cwd}
            </span>
          ) : null}
          {item.truncated ? <span class="badge badge-warn">truncated</span> : null}
          <span class="command-row-meta">
            {/* A command's stdio is what arrives while it runs, so its own
                throughput is measured on that - not on the model, which is
                idle until the command returns. */}
            <TpsBadge
              chars={item.stdout.length + item.stderr.length}
              working={item.status === 'running'}
              source="result"
            />
            <CommandOutcome item={item} />
            {duration ? <span class="command-duration">{duration}</span> : null}
            <span class="disclosure" aria-hidden="true">
              <ChevronDownIcon size={12} />
            </span>
          </span>
        </button>

        {open ? (
          <div class="cmd-output" id={bodyId}>
            <CommandOutput item={item} />
          </div>
        ) : null}
      </section>
    );
  }

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
        onClick={toggle}
      >
        <span class={`cmd-glyph ${tone}`} aria-hidden="true">
          <CommandGlyph item={item} ok={ok} />
        </span>
        <span class="command-text" title={item.command}>
          {item.command}
        </span>
        {cwd ? (
          <span class="command-cwd" title={cwd}>
            {cwd}
          </span>
        ) : null}
        {item.truncated ? <span class="badge badge-warn">truncated</span> : null}
        <TpsBadge
          chars={item.stdout.length + item.stderr.length}
          working={item.status === 'running'}
          source="result"
        />
        <CommandOutcome item={item} />
        {duration ? <span class="command-duration">{duration}</span> : null}
        <span class="disclosure" aria-hidden="true">
          <ChevronDownIcon size={12} />
        </span>
      </button>

      <div class="cmd-output" id={bodyId} hidden={!open}>
        {open ? <CommandOutput item={item} /> : null}
      </div>
    </section>
  );
}
