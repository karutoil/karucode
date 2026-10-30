import type { JSX } from 'preact';
import { useId, useState } from 'preact/hooks';
import { formatDuration } from '../lib/format';
import type { CommandItem, ToolItem } from '../state/chat';
import { callTotals, callsSummary, type CallItem } from '../state/transcriptLayout';
import { CommandCard } from './CommandCard';
import { ToolCard } from './ToolCard';
import { AlertIcon, CheckIcon, ChevronDownIcon } from './icons';

export interface ToolGroupProps {
  calls: CallItem[];
  /** How each call is drawn inside the group (the transcript's tool style). */
  variant: 'card' | 'row';
}

/** The group's one-line verdict: outcome glyph plus "3 calls · 1.2s". */
function groupTone(totals: ReturnType<typeof callTotals>): string {
  if (totals.running > 0) return 'is-running';
  if (totals.failed > 0) return 'is-error';
  if (totals.stopped > 0) return 'is-stopped';
  return 'is-ok';
}

function GroupGlyph({ totals }: { totals: ReturnType<typeof callTotals> }): JSX.Element {
  if (totals.running > 0) return <span class="status-dot status-running" aria-hidden="true" />;
  if (totals.failed > 0) return <AlertIcon size={13} />;
  if (totals.stopped > 0) return <span class="status-dot status-stopped" aria-hidden="true" />;
  return <CheckIcon size={13} />;
}

/**
 * A run of consecutive tool calls folded into one line.
 *
 * The summary says what happened ("read_file, grep +1 more · 4 calls · 1.2s")
 * and opens to the individual calls, each with its own output disclosure. It
 * opens itself while a member is still running or after a failure: those are the
 * two states a reader must not have to click to discover.
 */
export function ToolGroup({ calls, variant }: ToolGroupProps): JSX.Element {
  const totals = callTotals(calls);
  const [open, setOpen] = useState(totals.running > 0 || totals.failed > 0);
  const bodyId = useId();
  const tone = groupTone(totals);
  const duration = formatDuration(totals.durationMs);
  const summary = callsSummary(totals);

  return (
    <section class="tool-group" data-tone={tone} data-open={open ? 'true' : 'false'} aria-label="Tool calls">
      <button
        type="button"
        class="tool-group-head"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => setOpen((previous) => !previous)}
      >
        <span class={`tool-group-glyph ${tone}`} aria-hidden="true">
          <GroupGlyph totals={totals} />
        </span>
        <span class="tool-group-summary">{summary}</span>
        <span class="tool-group-meta">
          <span class="tool-group-count">
            {totals.count} {totals.count === 1 ? 'call' : 'calls'}
          </span>
          {totals.running > 0 ? (
            <span class="tool-group-running">{totals.running} running</span>
          ) : null}
          {totals.failed > 0 ? (
            <span class="tool-group-failed">
              {totals.failed} failed
            </span>
          ) : null}
          {duration ? <span class="tool-group-duration">{duration}</span> : null}
          <span class="disclosure" aria-hidden="true">
            <ChevronDownIcon size={12} />
          </span>
        </span>
      </button>

      <div class="tool-group-body" id={bodyId} hidden={!open}>
        {open
          ? calls.map((call) =>
              call.kind === 'command' ? (
                <CommandCard
                  key={call.id}
                  item={call as CommandItem}
                  defaultOpen={call.status === 'running'}
                />
              ) : (
                <ToolCard key={call.id} item={call as ToolItem} variant={variant} />
              ),
            )
          : null}
      </div>
    </section>
  );
}
