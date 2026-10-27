import type { JSX } from 'preact';
import { formatDuration, jsonSummary } from '../lib/format';
import type { ToolItem } from '../state/chat';
import { toolStatusLabel } from '../state/selectors';
import { ChevronRightIcon } from './icons';

export interface ToolCardProps {
  item: ToolItem;
  defaultOpen?: boolean;
}

export function ToolCard({ item, defaultOpen = false }: ToolCardProps): JSX.Element {
  const detail =
    typeof item.args === 'string' ? item.args : JSON.stringify(item.args);
  const summary = jsonSummary(item.args, 110) || 'no arguments';
  const status = toolStatusLabel(item.status);
  const duration = formatDuration(item.durationMs);

  return (
    <details class="tool-card" data-status={item.status} open={defaultOpen}>
      <summary class="tool-row">
        <span class={`status-dot status-${item.status}`} aria-hidden="true" />
        <span class="tool-name">{item.name}</span>
        <span class="tool-args" title={detail.length > 500 ? `${detail.slice(0, 500)}…` : detail}>
          {summary}
        </span>
        {item.truncated ? <span class="badge badge-warn">truncated</span> : null}
        <span class="tool-meta">
          {status}
          {duration ? ` · ${duration}` : ''}
        </span>
        <span class="disclosure" aria-hidden="true">
          <ChevronRightIcon size={13} />
        </span>
      </summary>
      <div class="tool-output">
        {item.output.length > 0 ? (
          <pre class="output-pre">{item.output}</pre>
        ) : (
          <p class="muted small">{item.status === 'running' ? 'Waiting for output…' : 'No output.'}</p>
        )}
      </div>
    </details>
  );
}