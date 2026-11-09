import type { JSX } from 'preact';
import { useEffect, useId, useRef, useState } from 'preact/hooks';
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
  /**
   * Whether the group starts expanded — the theme's `working` option. A group is
   * its own disclosure either way: opening this run of calls never opens another.
   */
  defaultOpen?: boolean;
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
 * two states a reader must not have to click to discover. `defaultOpen` is the
 * theme's **Working steps** option, so a transcript asked to show the work shows
 * the calls inside a run too rather than stopping at another summary line.
 *
 * The run is live - calls arrive, start and fail after this group is mounted - so
 * the initial state is not enough. The effect below watches the *shape* of the
 * trouble (how many are running, how many failed) and opens on any change into a
 * troubled shape. It never closes the group again: a run that failed and then
 * settled leaves its output open, because that output is the evidence the reader
 * opened it for. Nothing runs on a shape the reader collapsed by hand while the
 * same call is still in flight - only a *new* running/failed member does.
 */
export function ToolGroup({ calls, variant, defaultOpen = false }: ToolGroupProps): JSX.Element {
  const totals = callTotals(calls);
  const urgent = totals.running > 0 || totals.failed > 0;
  // `choice` is what the reader clicked; `trouble` latches the automatic opening
  // so a group that failed and then settled stays open. Until either exists the
  // theme's option decides, which is what makes switching it repaint every group.
  const [choice, setChoice] = useState<boolean | null>(null);
  const [trouble, setTrouble] = useState(urgent);
  const open = choice ?? (defaultOpen || trouble);
  const previousStatuses = useRef(
    new Map(calls.map((call) => [call.id, call.status] as const)),
  );
  const bodyId = useId();
  const tone = groupTone(totals);
  const duration = formatDuration(totals.durationMs);
  const summary = callsSummary(totals);

  // Counts alone are not a transition: one running call can finish as another
  // starts, and a reader can deliberately collapse the remaining work. Compare
  // the status of each call by id so only new work or an actual failure reopens
  // the group; a mere decrease in running count leaves the reader's choice alone.
  useEffect(() => {
    const newlyUrgent = calls.some((call) => {
      if (call.status !== 'running' && call.status !== 'error') return false;
      return previousStatuses.current.get(call.id) !== call.status;
    });
    previousStatuses.current = new Map(calls.map((call) => [call.id, call.status] as const));
    if (newlyUrgent) {
      // A call lit up: open, and drop a collapse the reader made before it
      // existed, because the new call is not the one they were looking at.
      setTrouble(true);
      setChoice(null);
    }
  }, [calls]);

  return (
    <section class="tool-group" data-tone={tone} data-open={open ? 'true' : 'false'} aria-label="Tool calls">
      <button
        type="button"
        class="tool-group-head"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => setChoice(!open)}
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
                  variant={variant}
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
