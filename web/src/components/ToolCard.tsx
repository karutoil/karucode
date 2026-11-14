import type { JSX } from 'preact';
import { useEffect, useId, useRef, useState } from 'preact/hooks';
import type { ImageAttachment } from '../api/types';
import { formatDuration, jsonSummary } from '../lib/format';
import { imageDataUrl, imageLabel } from '../lib/images';
import type { ToolItem } from '../state/chat';
import { toolStatusLabel } from '../state/selectors';
import { Dialog } from './Dialog';
import { TpsBadge } from './TpsBadge';
import {
  AlertIcon,
  BoltIcon,
  CheckIcon,
  ChevronDownIcon,
  FileCodeIcon,
  SearchIcon,
  TerminalIcon,
} from './icons';

export interface ToolCardProps {
  item: ToolItem;
  defaultOpen?: boolean;
  /**
   * `card` is the roomy mock card; `row` is the compact one-line call that
   * expands into the same output. The transcript picks it from the theme
   * (`ThemeToolStyle`), so a long turn can read as a list.
   */
  variant?: 'card' | 'row';
}

/** Above this the numbered preview stops rendering lines; the tool was truncated anyway. */
const MAX_RENDERED_LINES = 2000;

/** Picks a symbol for the tool by name, so a glance tells you what ran. */
function ToolSymbol({ name }: { name: string }): JSX.Element {
  const normalised = name.toLowerCase();
  if (/(shell|command|exec|bash|term)/.test(normalised)) return <TerminalIcon size={15} />;
  if (/(grep|glob|search|list|find)/.test(normalised)) return <SearchIcon size={15} />;
  if (/(read|write|edit|file|patch)/.test(normalised)) return <FileCodeIcon size={15} />;
  return <BoltIcon size={15} />;
}

function StatusGlyph({ status }: { status: ToolItem['status'] }): JSX.Element {
  if (status === 'ok') return <CheckIcon size={12} />;
  if (status === 'error') return <AlertIcon size={12} />;
  return <span class={`status-dot status-${status}`} aria-hidden="true" />;
}

/**
 * The footer's one-line verdict, e.g. "Finished in 1.31s" — the mock puts the
 * duration here rather than in the header so the header stays about the call.
 */
function footerNote(item: ToolItem, duration: string): string {
  if (item.status === 'running') return duration ? `Running · ${duration}` : 'Running…';
  if (item.status === 'error') return duration ? `Failed in ${duration}` : 'Failed';
  if (item.status === 'stopped') return duration ? `Stopped after ${duration}` : 'Stopped';
  return duration ? `Finished in ${duration}` : 'Finished';
}

function outputLines(output: string): string[] {
  const normalised = output.replace(/\r\n/g, '\n');
  if (normalised === '') return [];
  const lines = normalised.split('\n');
  // A trailing newline is not an extra empty line.
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
  return lines;
}

/**
 * The images a tool attached for the model.
 *
 * Shown between the header and the (clamped) output, so a screenshot the agent
 * read is visible without expanding anything: that is the point of reading an
 * image. A thumbnail opens a lightbox - the chat column is too narrow to judge
 * a UI screenshot from a 160px-high card.
 */
function ToolImages({ images, tool }: { images: ImageAttachment[]; tool: string }): JSX.Element {
  const [enlarged, setEnlarged] = useState<number | null>(null);
  const current = enlarged === null ? null : images[enlarged];
  return (
    <div
      class="tool-images"
      role="group"
      aria-label={`${tool} returned ${images.length} ${images.length === 1 ? 'image' : 'images'}`}
    >
      {images.map((image, index) => {
        const src = imageDataUrl(image);
        const label = imageLabel(image);
        return (
          <figure class="tool-image" key={`${image.name ?? 'image'}-${index}`}>
            {src ? (
              <button
                type="button"
                class="tool-image-button"
                onClick={() => setEnlarged(index)}
                aria-label={`Enlarge ${label}`}
              >
                <img src={src} alt={label} loading="lazy" data-testid="tool-image" />
              </button>
            ) : (
              <p class="muted small tool-image-note">
                {image.media_type || 'image'} cannot be previewed here
              </p>
            )}
            <figcaption class="tool-image-caption">{label}</figcaption>
          </figure>
        );
      })}
      <Dialog
        open={current !== null}
        title={current ? imageLabel(current) : 'Image'}
        onClose={() => setEnlarged(null)}
        class="image-dialog"
      >
        {current ? (
          <img class="dialog-image" src={imageDataUrl(current) ?? ''} alt={imageLabel(current)} />
        ) : null}
      </Dialog>
    </div>
  );
}

/**
 * The output body shared by both variants: bounded numbered lines with the
 * "no output"/"waiting" states spelled out. It is mounted by disclosure;
 * a running card is the one live-preview exception.
 */
function ToolOutput({
  item,
  open,
  bodyId,
}: {
  item: ToolItem;
  open: boolean;
  bodyId: string;
}): JSX.Element {
  const lines = outputLines(item.output);
  const rendered = lines.slice(0, MAX_RENDERED_LINES);
  const hiddenLines = lines.length - rendered.length;
  const waiting = item.status === 'running' && lines.length === 0;
  const width = String(Math.max(rendered.length, 10)).length;

  return (
    <div
      class="tool-body"
      id={bodyId}
      role="group"
      aria-label={`${item.name} output`}
      tabIndex={open ? 0 : -1}
    >
      {waiting ? (
        <p class="muted small output-empty">Waiting for output…</p>
      ) : lines.length === 0 ? (
        <p class="muted small output-empty">No output.</p>
      ) : (
        <>
          {rendered.map((line, index) => (
            <div class="tool-line" key={index}>
              <span class="num" aria-hidden="true">
                {String(index + 1).padStart(width, '0')}
              </span>
              <span class="line-text">{line === '' ? '\u00a0' : line}</span>
            </div>
          ))}
          {hiddenLines > 0 ? (
            <p class="muted small output-empty">… {hiddenLines} more lines not shown</p>
          ) : null}
        </>
      )}
    </div>
  );
}

/**
 * A tool call in the assistant column: header (symbol, call, outcome) and a
 * footer with the duration and Show output disclosure. Successful output is
 * mounted only after expansion, so a settled call does not spend 112px on a raw
 * preview. Images stay visible outside that disclosure; failures open to their
 * explanation. Running cards retain their live output preview.
 *
 * With `variant="row"` the whole call is one line — symbol, name, arguments,
 * outcome and duration — and the same output opens beneath it. That is the
 * "Rows" transcript view: a turn with twenty calls reads as a list instead of
 * twenty blocks, while every byte of output is still one click away.
 */
export function ToolCard({ item, defaultOpen = false, variant = 'card' }: ToolCardProps): JSX.Element {
  const [open, setOpen] = useState(defaultOpen || item.status === 'error');
  const previousStatus = useRef(item.status);
  const bodyId = useId();

  // A failure carries its own explanation: open it on arrival instead of
  // reducing it to a generic status word. A deliberate manual close stays
  // closed until a different call state transitions into a new failure.
  useEffect(() => {
    const previous = previousStatus.current;
    previousStatus.current = item.status;
    if (item.status === 'error' && previous !== 'error') setOpen(true);
  }, [item.status]);

  const detail = typeof item.args === 'string' ? item.args : JSON.stringify(item.args);
  const summary = jsonSummary(item.args, variant === 'row' ? 90 : 110);
  const duration = formatDuration(item.durationMs);
  const lines = outputLines(item.output);
  const hasOutput = lines.length > 0;

  if (variant === 'row') {
    return (
      <section
        class="tool-row"
        data-status={item.status}
        data-open={open ? 'true' : 'false'}
        aria-label={`${item.name} tool call`}
      >
        <button
          type="button"
          class="tool-row-head"
          aria-expanded={open}
          aria-controls={bodyId}
          aria-label={
            summary !== ''
              ? `${item.name} ${summary} ${toolStatusLabel(item.status)}`
              : `${item.name} ${toolStatusLabel(item.status)}`
          }
          title={detail.length > 500 ? `${detail.slice(0, 500)}…` : detail}
          onClick={() => setOpen((previous) => !previous)}
        >
          <span class="tool-row-symbol" aria-hidden="true">
            <ToolSymbol name={item.name} />
          </span>
          <span class="tool-row-name">{item.name}</span>
          {summary !== '' ? <span class="tool-row-args">{summary}</span> : null}
          {item.truncated ? <span class="badge badge-warn">truncated</span> : null}
          <span class="tool-row-meta">
            {duration ? <span class="tool-row-duration">{duration}</span> : null}
            {/* The call's own output is the only text it produces while it
                runs, so that is what its throughput measures. */}
            <TpsBadge chars={item.output.length} working={item.status === 'running'} source="result" />
            <span class={`tool-row-status tool-success-${item.status}`}>
              <StatusGlyph status={item.status} />
              {toolStatusLabel(item.status)}
            </span>
            <span class="disclosure" aria-hidden="true">
              <ChevronDownIcon size={12} />
            </span>
          </span>
        </button>

        {item.images && item.images.length > 0 ? (
          <ToolImages images={item.images} tool={item.name} />
        ) : null}

        {/* Mounted only while open: a collapsed row should cost nothing, and the
            disclosure is wired to the same id the card variant uses. */}
        {open ? (
          <>
            <ToolOutput item={item} open={open} bodyId={bodyId} />
            {hasOutput ? (
              <p class="tool-row-foot">
                <span class="tool-duration">{footerNote(item, duration)}</span>
              </p>
            ) : null}
          </>
        ) : null}
      </section>
    );
  }

  return (
    <section
      class="tool-card"
      data-status={item.status}
      data-open={open ? 'true' : 'false'}
      aria-label={`${item.name} tool call`}
    >
      <header class="tool-head">
        <span class="tool-symbol" aria-hidden="true">
          <ToolSymbol name={item.name} />
        </span>
        <span class="tool-command" title={detail.length > 500 ? `${detail.slice(0, 500)}…` : detail}>
          <span class="tool-name">{item.name}</span>
          {summary !== '' ? <span class="tool-args">{summary}</span> : null}
        </span>
        {item.truncated ? <span class="badge badge-warn">truncated</span> : null}
        {/* Live output throughput for the call itself: a tool's result is the
            text it produces, so that is what the badge counts while it runs.
            It goes the moment the call settles, with the running state. */}
        <TpsBadge chars={item.output.length} working={item.status === 'running'} source="result" />
        <span class={`tool-success tool-success-${item.status}`}>
          <StatusGlyph status={item.status} />
          {toolStatusLabel(item.status)}
        </span>
      </header>

      {item.images && item.images.length > 0 ? (
        <ToolImages images={item.images} tool={item.name} />
      ) : null}

      {open || item.status === 'running' ? (
        <ToolOutput item={item} open={open} bodyId={bodyId} />
      ) : null}

      <div class="tool-foot">
        <span class="tool-duration">{footerNote(item, duration)}</span>
        <button
          type="button"
          class="tool-expand"
          aria-expanded={open}
          aria-controls={bodyId}
          onClick={() => setOpen((previous) => !previous)}
        >
          {open ? 'Show less' : lines.length > 0 ? 'Show output' : 'Show details'}
          <ChevronDownIcon size={12} />
        </button>
      </div>
    </section>
  );
}
