import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { truncate } from '../lib/format';
import { Badge, Button, Callout, Chip, Inline, Stack, Text } from '../ui';
import type { FeedbackTone } from '../ui';
import type { PluginPanelRenderProps, PluginPanelRenderer } from './pluginPanels';
import { InspectorEmpty } from './InspectorEmpty';
import {
  AlertIcon,
  CheckIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CircleCheckIcon,
} from './icons';

/** How long a task the agent just added keeps its "New" marker. Long enough to
 *  be noticed after the panel reveals it, short enough that it never reads as
 *  the task's actual state. */
const NEW_TASK_MARKER_MS = 8000;

/**
 * The renderer for the `kc-todo` plugin's `todo` panel.
 *
 * ## What this file is, and where it goes
 *
 * A plugin cannot ship UI: the frontend is a compile-time snapshot embedded in
 * the binary. A plugin declares a panel and answers `panels/read` with JSON; the
 * app decides how to draw it. Without this file the tab still works — the
 * generic JSON view renders the payload — but it reads as a data dump. This is
 * the "draw it nicely" half, and it is registered at startup with
 * `registerPluginPanelRenderer`.
 *
 * Copy it to `web/src/components/TodoPanel.tsx` and import it from `app.tsx`
 * (see README §4). It is kept here, next to the plugin, because the two are two
 * halves of one contract: `panel_todo()` in `src/main.rs` produces exactly the
 * payload this file consumes.
 *
 * ## The security contract
 *
 * `data` is `unknown` and is untrusted: it is whatever JSON the plugin process
 * returned. Everything below validates before reading a field, never reaches
 * `dangerouslySetInnerHTML`, and never builds a URL from it. A title is inserted
 * as a text node, so a title containing markup is shown as text.
 */

/** One item as the plugin's `to_view` serialises it. */
interface PanelItem {
  id: string;
  title: string;
  status: string;
  label: string;
  parent: string | null;
  depth: number;
  detail: string;
  evidence: string;
  limitation: string;
  reason: string;
  started: boolean;
  open: boolean;
  /** What the current status still needs, e.g. "`evidence` (what proves it)". */
  needs: string;
  children: string[];
}

interface PanelAudit {
  total: number;
  open: string[];
  in_progress: string[];
  unverified: string[];
  missing_evidence: string[];
  missing_limitation: string[];
  clean: boolean;
}

interface PanelCarryOver {
  run_id: string;
  at_ms: number;
  in_progress: string[];
  pending: string[];
  /**
   * The subset of `in_progress`/`pending` that is still open *now*.
   *
   * `undefined` when the payload predates the field (an older plugin version), so
   * the renderer can fall back to the raw ids and never silently hide a real
   * carry-over. `[]` is different: it means the plugin checked and nothing is
   * still open.
   */
  still_open?: string[];
}

/** The payload the plugin returns, once validated. */
interface TodoPayload {
  items: PanelItem[];
  audit: PanelAudit;
  carry_over: PanelCarryOver[];
  open_count: number;
  workspace: string;
  path: string;
  version: number;
  error?: string;
}

type Tone = 'active' | 'open' | 'warn' | 'done' | 'muted';

/** Most-actionable first: unfinished work reads before the completed pile. */
const STATUS_ORDER = ['in_progress', 'blocked', 'partial', 'pending', 'deferred', 'done', 'cancelled'];

const FALLBACK_LABELS: Record<string, string> = {
  pending: 'Not implemented',
  in_progress: 'In progress',
  blocked: 'Blocked',
  deferred: 'Deferred',
  partial: 'Implemented (limited)',
  done: 'Implemented',
  cancelled: 'Cancelled',
};

function toneFor(status: string): Tone {
  switch (status) {
    case 'done':
      return 'done';
    case 'in_progress':
      return 'active';
    case 'partial':
    case 'blocked':
      return 'warn';
    case 'cancelled':
    case 'deferred':
      return 'muted';
    default:
      return 'open';
  }
}

function feedbackTone(tone: Tone): FeedbackTone {
  switch (tone) {
    case 'active':
      return 'accent';
    case 'warn':
      return 'warn';
    case 'done':
      return 'success';
    case 'muted':
    case 'open':
      return 'neutral';
  }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function str(value: unknown): string {
  return typeof value === 'string' ? value : '';
}

function num(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

function idList(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((v): v is string => typeof v === 'string') : [];
}

function parseItem(value: unknown): PanelItem | null {
  if (!isRecord(value)) return null;
  const id = str(value.id);
  // An item without an id cannot be keyed, linked or referenced; drop it rather
  // than render a row the rest of the payload cannot point at.
  if (!id) return null;
  const status = str(value.status);
  return {
    id,
    title: str(value.title),
    status,
    label: str(value.label) || FALLBACK_LABELS[status] || status,
    parent: typeof value.parent === 'string' ? value.parent : null,
    depth: num(value.depth),
    detail: str(value.detail),
    evidence: str(value.evidence),
    limitation: str(value.limitation),
    reason: str(value.reason),
    started: value.started === true,
    open: value.open === true,
    needs: str(value.needs),
    children: idList(value.children),
  };
}

/**
 * Read the payload, or `undefined` when it is not the shape this renderer draws.
 *
 * Returning `undefined` is what the registry calls "declined": the shell falls
 * back to the generic JSON view instead of showing a broken or blank tab. That
 * matters because a future plugin version may change the payload, and an old
 * renderer must not blank the tab when it does.
 */
export function parseTodoPayload(data: unknown): TodoPayload | undefined {
  if (!isRecord(data)) return undefined;

  // A payload that only carries an error is still ours to draw: the tab shows
  // why the list could not be read.
  const error = typeof data.error === 'string' ? data.error : undefined;
  if (!Array.isArray(data.items)) {
    return error ? { items: [], audit: emptyAudit(), carry_over: [], open_count: 0, workspace: '', path: '', version: 0, error } : undefined;
  }

  const items = data.items.map(parseItem).filter((item): item is PanelItem => item !== null);
  const rawAudit = isRecord(data.audit) ? data.audit : {};
  const carryOver: PanelCarryOver[] = Array.isArray(data.carry_over)
    ? data.carry_over.filter(isRecord).map((run) => ({
        run_id: str(run.run_id),
        at_ms: num(run.at_ms),
        in_progress: idList(run.in_progress),
        pending: idList(run.pending),
        // Absent and empty mean different things: absent is an older payload (fall
        // back to the raw ids), empty is "checked, nothing left open".
        still_open: Array.isArray(run.still_open) ? idList(run.still_open) : undefined,
      }))
    : [];

  return {
    items,
    audit: {
      total: num(rawAudit.total),
      open: idList(rawAudit.open),
      in_progress: idList(rawAudit.in_progress),
      unverified: idList(rawAudit.unverified),
      missing_evidence: idList(rawAudit.missing_evidence),
      missing_limitation: idList(rawAudit.missing_limitation),
      clean: rawAudit.clean === true,
    },
    carry_over: carryOver,
    open_count: num(data.open_count),
    workspace: str(data.workspace),
    path: str(data.path),
    version: num(data.version),
    error,
  };
}

function emptyAudit(): PanelAudit {
  return {
    total: 0,
    open: [],
    in_progress: [],
    unverified: [],
    missing_evidence: [],
    missing_limitation: [],
    clean: true,
  };
}

/**
 * Items in rows: each task immediately followed by its subtasks.
 *
 * Orphans (a parent that is not in the payload) are promoted to the top level,
 * and an item that is neither a root nor a child of one (a cycle) is still
 * shown, so a hand-edited list cannot hide work.
 */
export function rowOrder(items: PanelItem[]): PanelItem[] {
  const ids = new Set(items.map((item) => item.id));
  const roots = items.filter((item) => !item.parent || !ids.has(item.parent));
  const rows: PanelItem[] = [];
  for (const root of roots) {
    rows.push(root);
    for (const child of items) {
      if (child.parent === root.id) rows.push(child);
    }
  }
  const shown = new Set(rows.map((item) => item.id));
  for (const item of items) {
    if (!shown.has(item.id)) rows.push(item);
  }
  return rows;
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

function noteFor(item: PanelItem): { kind: string; text: string } | null {
  if (item.status === 'done' && item.evidence) return { kind: 'evidence', text: item.evidence };
  if (item.status === 'partial' && item.limitation) {
    return { kind: 'limitation', text: item.limitation };
  }
  if (
    (item.status === 'blocked' || item.status === 'deferred' || item.status === 'cancelled') &&
    item.reason
  ) {
    return { kind: 'reason', text: item.reason };
  }
  return null;
}

function ItemRow({
  item,
  isSubtask,
  isNew = false,
  reveal = false,
}: {
  item: PanelItem;
  isSubtask: boolean;
  /** Added to the list since this panel last read it (LS-13). */
  isNew?: boolean;
  /** The one new task the panel scrolls to; the rest are only marked. */
  reveal?: boolean;
}): JSX.Element {
  const [open, setOpen] = useState(false);
  const rowRef = useRef<HTMLLIElement>(null);
  const note = noteFor(item);
  const expandable = Boolean(item.detail) || Boolean(note);
  const tone = toneFor(item.status);
  const bodyId = `todo-details-${item.id}`;
  // A list of ten "Show details" buttons tells a screen-reader user nothing
  // about which task each one belongs to. The id is the stable handle; the title
  // is what the reader recognises, so both go in the name.
  const task = item.title ? `${item.id}: ${truncate(item.title, 60)}` : item.id;

  // The list is in plan order, so an item the agent just added can land anywhere
  // in it — including below the fold of a panel the reader is already looking
  // at. One reveal, on the render that first marks it (LS-13).
  useEffect(() => {
    if (!reveal) return;
    const row = rowRef.current;
    if (row && typeof row.scrollIntoView === 'function') {
      row.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    }
  }, [reveal]);

  return (
    <li
      ref={rowRef}
      class={`todo-item todo-tone-${tone}${isSubtask ? ' is-subtask' : ''}${isNew ? ' is-new' : ''}`}
    >
      <div class="todo-item-head">
        {isNew ? (
          <span class="todo-new-flag">
            <span class="sr-only">Added just now. </span>New
          </span>
        ) : null}
        {expandable ? (
          <button
            type="button"
            class="todo-item-toggle"
            onClick={() => setOpen((value) => !value)}
            aria-expanded={open}
            // The body is always in the DOM and merely hidden, so the reference
            // is valid in both states instead of pointing at nothing when closed.
            aria-controls={bodyId}
            aria-label={`${open ? 'Hide' : 'Show'} details for ${task}`}
          >
            {open ? <ChevronDownIcon size={11} /> : <ChevronRightIcon size={11} />}
          </button>
        ) : (
          <span class="todo-item-toggle is-static" aria-hidden="true">
            <span class="todo-item-dot" />
          </span>
        )}

        <Badge tone={feedbackTone(tone)} mono={false} class={`todo-status todo-tone-${tone}`}>
          {item.status === 'done' ? <CheckIcon size={11} /> : null}
          {item.label}
        </Badge>

        <Text as="span" mono size="tiny" tone="muted" class="todo-id">
          {item.id}
        </Text>
        {/* A text node, so a title containing markup is shown as text. */}
        <Text
          as="span"
          size="small"
          tone={tone === 'done' || tone === 'muted' || tone === 'open' ? 'muted' : 'default'}
          class="todo-title"
        >
          {item.title}
        </Text>
      </div>

      <div class="todo-item-body" id={bodyId} hidden={!open}>
        {item.detail ? (
          <Text as="p" size="tiny" tone="muted" class="todo-detail">
            {item.detail}
          </Text>
        ) : null}
        {note ? (
          <p class="todo-note">
            <Text as="span" size="micro" weight="bold" tone="muted" class="todo-note-kind">
              {note.kind}
            </Text>
            {note.text}
          </p>
        ) : null}
      </div>
    </li>
  );
}

function AuditStrip({ audit }: { audit: PanelAudit }): JSX.Element | null {
  const problems: string[] = [];
  if (audit.unverified.length > 0) {
    problems.push(
      `${audit.unverified.length} marked implemented without ever being in progress (${audit.unverified.join(', ')})`,
    );
  }
  if (audit.missing_evidence.length > 0) {
    problems.push(
      `${audit.missing_evidence.length} implemented with no evidence (${audit.missing_evidence.join(', ')})`,
    );
  }
  if (audit.missing_limitation.length > 0) {
    problems.push(
      `${audit.missing_limitation.length} limited with no stated limitation (${audit.missing_limitation.join(', ')})`,
    );
  }

  if (problems.length === 0) {
    if (audit.total === 0) return null;
    return (
      <Callout
        tone="success"
        role="status"
        icon={<CircleCheckIcon size={12} />}
        class="todo-audit todo-audit-clean"
      >
        No unverified claims.
      </Callout>
    );
  }

  return (
    <Callout
      tone="warn"
      role="status"
      title="Needs a second look"
      icon={<AlertIcon size={12} />}
      class="todo-audit todo-audit-warn"
    >
      <ul class="todo-audit-list">
        {problems.map((problem) => (
          <li key={problem}>{problem}</li>
        ))}
      </ul>
    </Callout>
  );
}

/**
 * The Todo panel body. The shell (`PluginPanelView`) already draws the header,
 * the refresh button and the "read from the plugin at …" footer, so this renders
 * only the list itself.
 */
export function TodoPanel({ data, refresh }: PluginPanelRenderProps): JSX.Element {
  const payload = parseTodoPayload(data);
  // Which tasks this panel had already drawn, and which of the current ones the
  // agent added since. Both are refs/state above the early returns because they
  // have to survive every one of them (hooks are not conditional).
  const seen = useRef<Set<string> | null>(null);
  const [newIds, setNewIds] = useState<string[]>([]);
  const listKey = (payload?.items ?? []).map((item) => item.id).join('\u0000');

  useEffect(() => {
    const ids = listKey === '' ? [] : listKey.split('\u0000');
    const before = seen.current;
    seen.current = new Set(ids);
    // The first read is the panel opening, not news: marking every task "new"
    // because the tab mounted would be a lie the reader would see through.
    if (before === null) return undefined;
    const added = ids.filter((id) => !before.has(id));
    if (added.length === 0) return undefined;
    setNewIds(added);
    // Long enough to read, short enough that it cannot look like current state.
    const timer = window.setTimeout(() => setNewIds([]), NEW_TASK_MARKER_MS);
    return () => window.clearTimeout(timer);
  }, [listKey]);

  // `matches` below guarantees this, but a renderer must never throw: a bad
  // payload should degrade, not blank the inspector.
  if (!payload) {
    return (
      <Callout tone="danger" role="alert" title="Unrecognised task list" class="empty-inspector">
        The kc-todo plugin returned a payload this renderer cannot read.
      </Callout>
    );
  }

  if (payload.error) {
    return (
      <Callout
        tone="warn"
        role="alert"
        title="Could not read the list"
        icon={<AlertIcon size={12} />}
        class="todo-audit todo-audit-warn"
        // A failed read is the one state with an action: the file may be
        // half-written while the agent is editing it, so reading again is the
        // whole of the recovery and it should not need the panel's own refresh.
        actions={
          <Button variant="secondary" size="sm" onClick={refresh}>
            Try again
          </Button>
        }
      >
        <Text as="p" size="tiny" mono tone="secondary" class="todo-error-text">
          {payload.error}
        </Text>
      </Callout>
    );
  }

  if (payload.items.length === 0) {
    return (
      <InspectorEmpty title="No plan yet">
        When the agent plans multi-step work it records a task list here, with subtasks and a
        status for each. Nothing appears until then.
      </InspectorEmpty>
    );
  }

  const counts = new Map<string, number>();
  for (const item of payload.items) {
    counts.set(item.status, (counts.get(item.status) ?? 0) + 1);
  }
  const summary = STATUS_ORDER.filter((status) => (counts.get(status) ?? 0) > 0);
  const rows = rowOrder(payload.items);
  const carry = payload.carry_over[payload.carry_over.length - 1];
  const openCount = payload.open_count || payload.audit.open.length;

  // Only report a carry-over that still matters. A run-end audit records what was
  // open *then*; if everything has since been closed, saying "left open by the
  // last run" would be false. `still_open` is the plugin's own check of the
  // current list, and an older payload without the field falls back to the raw
  // ids so the line is never silently dropped.
  const carryStillOpen = carry
    ? (carry.still_open ?? [...carry.in_progress, ...carry.pending])
    : [];
  const carryShow = carry !== undefined && carryStillOpen.length > 0;
  // Group by why each item was carried over, using the *current* list so an item
  // that has since moved on is described by what it was at run end, not by what
  // it happens to be now.
  const carryParts: string[] = [];
  if (carry && carryShow) {
    const inProgress = carryStillOpen.filter((id) => carry.in_progress.includes(id));
    const pending = carryStillOpen.filter((id) => !carry.in_progress.includes(id));
    if (inProgress.length > 0) carryParts.push(`${inProgress.join(', ')} still in progress`);
    if (pending.length > 0) carryParts.push(`${pending.join(', ')} never started`);
  }

  return (
    <Stack gap="4">
      <Inline gap="3" wrap class="todo-summary">
        {summary.map((status) => (
          <Chip
            key={status}
            tone={feedbackTone(toneFor(status))}
            class={`todo-count todo-tone-${toneFor(status)}`}
          >
            <Text as="span" size="tiny" mono weight="bold" class="todo-count-number">
              {counts.get(status)}
            </Text>{' '}
            <Text as="span" size="tiny" tone="muted" class="todo-count-label">
              {FALLBACK_LABELS[status] ?? status}
            </Text>
          </Chip>
        ))}
      </Inline>

      {carryShow ? (
        <Callout tone="warn" role="status" class="todo-carry">
          Still open from the last run: {carryParts.join('; ')}.
        </Callout>
      ) : null}

      <Stack as="ul" gap="1" class="todo-list">
        {rows.map((item) => (
          <ItemRow
            key={item.id}
            item={item}
            isSubtask={item.depth > 0 || Boolean(item.parent)}
            isNew={newIds.includes(item.id)}
            // One reveal, not one per added task: five scrolls race and the last
            // one wins, which is not the item the reader needs to see first.
            reveal={newIds[0] === item.id}
          />
        ))}
      </Stack>

      <AuditStrip audit={payload.audit} />

      <Text as="p" size="tiny" tone="muted" class="inspector-note">
        {openCount === 0 ? 'Everything is closed.' : `${openCount} still open.`} Written by the
        agent through <code>kc-todo</code>; read-only here so this list and the agent's cannot
        drift apart.
      </Text>
    </Stack>
  );
}

/**
 * The registry entry. `matches` runs before `render`, so it validates the shape
 * cheaply and declines anything else — the generic JSON view then shows it
 * instead of a broken tab.
 */
export const todoPanelRenderer: PluginPanelRenderer = {
  matches: (data) => parseTodoPayload(data) !== undefined,
  render: (props) => <TodoPanel {...props} />,
};

/**
 * The hosted id the inspector gives the panel (`plugin__<plugin>__<id>`).
 *
 * Registering the hosted id rather than the bare `todo` means another plugin
 * with a panel of the same name is unaffected by this renderer.
 */
export const TODO_PANEL_ID = 'plugin__kc-todo__todo';
