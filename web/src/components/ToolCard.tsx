import type { JSX } from 'preact';
import { useId, useState } from 'preact/hooks';
import { formatDuration, jsonSummary } from '../lib/format';
import type { ToolItem } from '../state/chat';
import { toolStatusLabel } from '../state/selectors';
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
 * A tool call in the assistant column: header (symbol, call, outcome), a
 * line-numbered output preview clamped to the mock's 112px, and a footer with
 * the duration and the Show output disclosure.
 */
export function ToolCard({ item, defaultOpen = false }: ToolCardProps): JSX.Element {
  const [open, setOpen] = useState(defaultOpen);
  const bodyId = useId();
  const detail = typeof item.args === 'string' ? item.args : JSON.stringify(item.args);
  const summary = jsonSummary(item.args, 110);
  const duration = formatDuration(item.durationMs);
  const lines = outputLines(item.output);
  const rendered = lines.slice(0, MAX_RENDERED_LINES);
  const hiddenLines = lines.length - rendered.length;
  const waiting = item.status === 'running' && lines.length === 0;
  const width = String(Math.max(rendered.length, 10)).length;

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
        <span class={`tool-success tool-success-${item.status}`}>
          <StatusGlyph status={item.status} />
          {toolStatusLabel(item.status)}
        </span>
      </header>

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
