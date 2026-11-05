import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { truncate } from '../lib/format';
import type { PluginPanelRenderProps, PluginPanelRenderer } from './pluginPanels';
import {
  AlertIcon,
  CheckIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CircleCheckIcon,
} from './icons';

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

function ItemRow({ item, isSubtask }: { item: PanelItem; isSubtask: boolean }): JSX.Element {
  const [open, setOpen] = useState(false);
  const note = noteFor(item);
  const expandable = Boolean(item.detail) || Boolean(note);
  const tone = toneFor(item.status);
  const bodyId = `todo-details-${item.id}`;
  // A list of ten "Show details" buttons tells a screen-reader user nothing
  // about which task each one belongs to. The id is the stable handle; the title
  // is what the reader recognises, so both go in the name.
  const task = item.title ? `${item.id}: ${truncate(item.title, 60)}` : item.id;

  return (
    <li class={`todo-item todo-tone-${tone}${isSubtask ? ' is-subtask' : ''}`}>
      <div class="todo-item-head">
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

        <span class={`todo-status todo-tone-${tone}`}>
          {item.status === 'done' ? <CheckIcon size={11} /> : null}
          {item.label}
        </span>

        <span class="todo-id">{item.id}</span>
        {/* A text node, so a title containing markup is shown as text. */}
        <span class="todo-title">{item.title}</span>
      </div>

      <div class="todo-item-body" id={bodyId} hidden={!open}>
        {item.detail ? <p class="todo-detail">{item.detail}</p> : null}
        {note ? (
          <p class="todo-note">
            <span class="todo-note-kind">{note.kind}</span>
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
      <p class="todo-audit todo-audit-clean">
        <CircleCheckIcon size={12} /> No unverified claims.
      </p>
    );
  }

  return (
    <div class="todo-audit todo-audit-warn">
      <div class="todo-audit-head">
        <AlertIcon size={12} />
        <span>Needs a second look</span>
      </div>
      <ul class="todo-audit-list">
        {problems.map((problem) => (
          <li key={problem}>{problem}</li>
        ))}
      </ul>
    </div>
  );
}

/**
 * The Todo panel body. The shell (`PluginPanelView`) already draws the header,
 * the refresh button and the "read from the plugin at …" footer, so this renders
 * only the list itself.
 */
export function TodoPanel({ data }: PluginPanelRenderProps): JSX.Element {
  const payload = parseTodoPayload(data);
  // `matches` below guarantees this, but a renderer must never throw: a bad
  // payload should degrade, not blank the inspector.
  if (!payload) {
    return (
      <div class="empty-inspector" role="alert">
        <p class="empty-inspector-title">Unrecognised task list</p>
        <p>The kc-todo plugin returned a payload this renderer cannot read.</p>
      </div>
    );
  }

  if (payload.error) {
    return (
      <div class="todo-audit todo-audit-warn" role="alert">
        <div class="todo-audit-head">
          <AlertIcon size={12} />
          <span>Could not read the list</span>
        </div>
        <p class="todo-error-text">{payload.error}</p>
      </div>
    );
  }

  if (payload.items.length === 0) {
    return (
      <div class="empty-inspector">
        <p class="empty-inspector-title">No plan yet</p>
        <p>
          When the agent plans multi-step work it records a task list here, with subtasks and a
          status for each. Nothing appears until then.
        </p>
      </div>
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
    <>
      <div class="todo-summary">
        {summary.map((status) => (
          <span key={status} class={`todo-count todo-tone-${toneFor(status)}`}>
            <span class="todo-count-number">{counts.get(status)}</span>
            <span class="todo-count-label">{FALLBACK_LABELS[status] ?? status}</span>
          </span>
        ))}
      </div>

      {carryShow ? (
        <p class="todo-carry">
          Still open from the last run: {carryParts.join('; ')}.
        </p>
      ) : null}

      <ul class="todo-list">
        {rows.map((item) => (
          <ItemRow key={item.id} item={item} isSubtask={item.depth > 0 || Boolean(item.parent)} />
        ))}
      </ul>

      <AuditStrip audit={payload.audit} />

      <p class="inspector-note">
        {openCount === 0 ? 'Everything is closed.' : `${openCount} still open.`} Written by the
        agent through <code>kc-todo</code>; read-only here so this list and the agent's cannot
        drift apart.
      </p>
    </>
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
