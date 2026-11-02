import type {
  AgentSummary,
  ApprovalDecision,
  ApprovalRequest,
  JsonValue,
  KarucodeEvent,
  SessionSummary,
  StoredMessage,
} from '../api/types';
import {
  normalizeAgentRecord,
  parseArgs,
  parseStoredCommandReport,
  stringArg,
  type ApprovalItem,
  type ApprovalView,
  type CommandItem,
  type MessageItem,
  type ToolItem,
} from './chat';

/**
 * Live transcript of one subagent.
 *
 * The parent chat deliberately ignores subagent text and tool events (they
 * belong to a different conversation). The Agents view keeps them here instead,
 * keyed by agent id, so "what is that subagent actually doing?" has an answer
 * without opening the raw database.
 */
export type AgentActivityItem = MessageItem | ToolItem | CommandItem | ApprovalItem;

export interface AgentActivity {
  items: AgentActivityItem[];
  /** Highest event `seq` applied, so a replayed frame cannot duplicate work. */
  lastSeq: number;
}

/** A running subagent can stream for hours; keep the tail, not the whole run. */
export const AGENT_ACTIVITY_LIMIT = 400;
/** One assistant message or tool result cannot grow past this in memory. */
const MAX_TEXT_CHARS = 60_000;

export function emptyActivity(): AgentActivity {
  return { items: [], lastSeq: 0 };
}

function clampText(text: string): string {
  if (text.length <= MAX_TEXT_CHARS) return text;
  return `${text.slice(0, MAX_TEXT_CHARS)}\n…(truncated in the live view)`;
}

function trimItems(items: AgentActivityItem[]): AgentActivityItem[] {
  return items.length > AGENT_ACTIVITY_LIMIT
    ? items.slice(items.length - AGENT_ACTIVITY_LIMIT)
    : items;
}

function isStreamingMessage(item: AgentActivityItem | undefined): item is MessageItem {
  return item?.kind === 'message' && item.streaming;
}

/**
 * The agent an event belongs to.
 *
 * The stream flattens payload fields into the envelope, and `agent_spawned`,
 * `agent_progress` and `agent_finished` all carry the child's id in the payload,
 * so `event.agent_id` is the child for every agent frame. Activity events carry
 * it in the envelope only, which resolves to the same field.
 */
export function agentEventId(event: KarucodeEvent): string | null {
  return event.agent_id || null;
}

export function isAgentLifecycleEvent(event: KarucodeEvent): boolean {
  return (
    event.type === 'agent_spawned' ||
    event.type === 'agent_progress' ||
    event.type === 'agent_paused' ||
    event.type === 'agent_message' ||
    event.type === 'agent_note' ||
    event.type === 'agent_finished'
  );
}

function approvalView(request: ApprovalRequest, at: number): ApprovalView {
  return {
    id: request.id,
    kind: request.kind,
    title: request.title,
    detail: request.detail,
    payload: request.payload,
    session_id: request.session_id,
    run_id: request.run_id,
    agent_id: request.agent_id ?? null,
    status: 'pending',
    decision: null,
    created_at: at,
  };
}

function appendDelta(
  activity: AgentActivity,
  event: { seq: number; ts_ms: number; run_id?: string | null },
  agentId: string,
  field: 'content' | 'reasoning',
  delta: string,
): AgentActivity {
  const items = activity.items;
  const tail = items[items.length - 1];
  if (isStreamingMessage(tail)) {
    const next: MessageItem = {
      ...tail,
      [field]: clampText(tail[field] + delta),
      streaming: true,
    };
    return {
      items: [...items.slice(0, -1), next],
      lastSeq: Math.max(activity.lastSeq, event.seq),
    };
  }
  const item: MessageItem = {
    kind: 'message',
    id: `agent:${agentId}:${event.seq}`,
    at: event.ts_ms,
    role: 'assistant',
    content: field === 'content' ? clampText(delta) : '',
    reasoning: field === 'reasoning' ? clampText(delta) : '',
    streaming: true,
    runId: event.run_id ?? null,
    agentId,
  };
  return {
    items: trimItems([...items, item]),
    lastSeq: Math.max(activity.lastSeq, event.seq),
  };
}

function upsertByCallId(
  activity: AgentActivity,
  callId: string,
  event: { seq: number },
  build: () => ToolItem | CommandItem,
  update: (item: ToolItem | CommandItem) => ToolItem | CommandItem,
): AgentActivity {
  const index = activity.items.findIndex(
    (item) => (item.kind === 'tool' || item.kind === 'command') && item.callId === callId,
  );
  if (index === -1) {
    return {
      items: trimItems([...activity.items, build()]),
      lastSeq: Math.max(activity.lastSeq, event.seq),
    };
  }
  const current = activity.items[index];
  if (!current || (current.kind !== 'tool' && current.kind !== 'command')) return activity;
  const items = [...activity.items];
  items[index] = update(current);
  return { items, lastSeq: Math.max(activity.lastSeq, event.seq) };
}

/**
 * Apply one agent-scoped event to that agent's activity feed.
 *
 * Events for other agents and for the parent run are ignored, so the caller can
 * hand every frame it sees to this reducer.
 */
export function applyAgentEvent(activity: AgentActivity, event: KarucodeEvent): AgentActivity {
  const agentId = agentEventId(event);
  if (!agentId) return activity;
  const runId = event.run_id ?? null;
  const at = event.ts_ms;

  switch (event.type) {
    case 'text_delta': {
      if (!event.delta) return activity;
      return appendDelta(activity, event, agentId, 'content', event.delta);
    }

    case 'reasoning_delta': {
      if (!event.delta) return activity;
      return appendDelta(activity, event, agentId, 'reasoning', event.delta);
    }

    case 'tool_call_started': {
      return upsertByCallId(
        activity,
        event.call_id,
        event,
        () => ({
          kind: 'tool',
          id: `tool:${event.call_id}`,
          at,
          callId: event.call_id,
          name: event.name,
          args: event.arguments,
          status: 'running',
          output: '',
          truncated: false,
          durationMs: null,
          runId,
        }),
        (item) =>
          item.kind === 'tool'
            ? { ...item, name: event.name, args: event.arguments, status: 'running' }
            : item,
      );
    }

    case 'command_started': {
      return upsertByCallId(
        activity,
        event.call_id,
        event,
        () => ({
          kind: 'command',
          id: `cmd:${event.call_id}`,
          at,
          callId: event.call_id,
          command: event.command,
          cwd: event.cwd,
          stdout: '',
          stderr: '',
          output: '',
          exitCode: null,
          status: 'running',
          truncated: false,
          durationMs: null,
          runId,
        }),
        (item) => {
          if (item.kind === 'command') {
            return { ...item, command: event.command, cwd: event.cwd, status: 'running' };
          }
          return {
            kind: 'command',
            id: item.id,
            at: item.at,
            callId: item.callId,
            command: event.command,
            cwd: event.cwd,
            stdout: '',
            stderr: '',
            output: '',
            exitCode: null,
            status: 'running',
            truncated: false,
            durationMs: null,
            runId: item.runId,
          };
        },
      );
    }

    case 'command_output': {
      const target = event.stream === 'stderr' ? 'stderr' : 'stdout';
      const index = activity.items.findIndex(
        (item) => item.kind === 'command' && item.callId === event.call_id,
      );
      const current = index === -1 ? undefined : activity.items[index];
      if (!current || current.kind !== 'command') return activity;
      const items = [...activity.items];
      items[index] = {
        ...current,
        [target]: clampText(current[target] + event.chunk),
      };
      return { items, lastSeq: Math.max(activity.lastSeq, event.seq) };
    }

    case 'command_finished': {
      return upsertByCallId(
        activity,
        event.call_id,
        event,
        () => ({
          kind: 'command',
          id: `cmd:${event.call_id}`,
          at,
          callId: event.call_id,
          command: '',
          cwd: '',
          stdout: '',
          stderr: '',
          output: '',
          exitCode: event.exit_code,
          status: event.exit_code === 0 ? 'ok' : 'error',
          truncated: event.truncated,
          durationMs: event.duration_ms,
          runId,
        }),
        (item) =>
          item.kind === 'command'
            ? {
                ...item,
                exitCode: event.exit_code,
                durationMs: event.duration_ms,
                truncated: item.truncated || event.truncated,
                status: event.exit_code === 0 ? 'ok' : 'error',
              }
            : item,
      );
    }

    case 'tool_call_finished': {
      const output = clampText(event.output);
      return upsertByCallId(
        activity,
        event.call_id,
        event,
        () => ({
          kind: 'tool',
          id: `tool:${event.call_id}`,
          at,
          callId: event.call_id,
          name: event.name,
          args: {},
          status: event.ok ? 'ok' : 'error',
          output,
          truncated: event.truncated,
          durationMs: event.duration_ms,
          runId,
        }),
        (item) => {
          if (item.kind === 'command') {
            return {
              ...item,
              durationMs: event.duration_ms,
              truncated: item.truncated || event.truncated,
              status: event.ok ? item.status : 'error',
              stdout: item.stdout === '' ? output : item.stdout,
            };
          }
          return {
            ...item,
            status: event.ok ? 'ok' : 'error',
            output,
            truncated: event.truncated,
            durationMs: event.duration_ms,
          };
        },
      );
    }

    case 'approval_requested': {
      const view = approvalView(event.approval, at);
      const existing = activity.items.find(
        (item) => item.kind === 'approval' && item.approval.id === view.id,
      );
      if (existing) {
        const items = activity.items.map((item) =>
          item.id === existing.id
            ? { ...(item as ApprovalItem), approval: view }
            : item,
        );
        return { items, lastSeq: Math.max(activity.lastSeq, event.seq) };
      }
      return {
        items: trimItems([
          ...activity.items,
          { kind: 'approval', id: `approval:${view.id}`, at, approval: view },
        ]),
        lastSeq: Math.max(activity.lastSeq, event.seq),
      };
    }

    case 'approval_resolved': {
      const decision: ApprovalDecision = event.decision;
      let changed = false;
      const items = activity.items.map((item) => {
        if (item.kind !== 'approval' || item.approval.id !== event.approval_id) return item;
        changed = true;
        return {
          ...item,
          approval: {
            ...item.approval,
            status: (decision === 'expired' ? 'expired' : 'resolved') as ApprovalView['status'],
            decision,
          },
        };
      });
      return changed ? { items, lastSeq: Math.max(activity.lastSeq, event.seq) } : activity;
    }

    case 'agent_finished': {
      const items = activity.items.map((item) => {
        if (isStreamingMessage(item)) return { ...item, streaming: false };
        if ((item.kind === 'tool' || item.kind === 'command') && item.status === 'running') {
          return { ...item, status: 'stopped' as const };
        }
        return item;
      });
      return { items, lastSeq: Math.max(activity.lastSeq, event.seq) };
    }

    default:
      return activity;
  }
}

function sessionContext(session: SessionSummary | undefined): {
  session_title: string;
  workspace: string;
  project_id: string | null;
} {
  return {
    session_title: session?.title ?? '',
    workspace: session?.workspace ?? '',
    project_id: session?.project_id ?? null,
  };
}

/**
 * A first row for an agent the server has not described yet.
 *
 * The SSE stream is global, so an `agent_progress`, `agent_paused` or
 * `agent_note` frame can reach the page **before** `GET /api/agents` answers on a
 * cold tab. Dropping those frames (`if (!current) return agents`) lost the fact
 * that the agent existed at all: the row then never appeared until a refresh, and
 * an early pause left it rendering `running` once the list arrived. `agent_finished`
 * already synthesised a row in exactly this situation; this is the same rule for
 * the three events that used to bail out.
 *
 * Only the id, the session and the event's own facts are filled in - everything
 * the frame does not carry is left at a neutral default, which the next
 * `GET /api/agents` (or the agent's `agent_spawned`) replaces.
 */
function placeholderAgent(
  id: string,
  event: { ts_ms: number; session_id?: string | null },
  session: SessionSummary | undefined,
  fields: Partial<AgentSummary> & Pick<AgentSummary, 'status'>,
): AgentSummary {
  return {
    id,
    session_id: event.session_id ?? '',
    parent_run_id: null,
    parent_agent_id: null,
    name: 'subagent',
    role: 'subagent',
    task: '',
    model: '',
    iterations: 0,
    summary: '',
    error: null,
    owned_paths: [],
    created_at: event.ts_ms,
    updated_at: event.ts_ms,
    finished_at: null,
    background: false,
    note: '',
    progress: null,
    ...sessionContext(session),
    ...fields,
  };
}

/**
 * Keep the workspace-wide agent list in step with live lifecycle events.
 *
 * Works for agents in sessions the user is not looking at: the SSE stream is
 * global, so a running subagent appears here even before its session is opened.
 */
export function applyAgentRegistry(
  agents: AgentSummary[],
  event: KarucodeEvent,
  sessions: SessionSummary[],
): AgentSummary[] {
  const id = agentEventId(event);
  if (!id) return agents;
  const index = agents.findIndex((agent) => agent.id === id);
  const current = index >= 0 ? agents[index] : undefined;
  const session = event.session_id
    ? sessions.find((candidate) => candidate.id === event.session_id)
    : undefined;
  // The list is scoped to the open project. A spawn in another project is not in
  // `sessions`, so it must not leak into this list; events for agents already
  // listed (a project switch mid-run) still update.
  if (index === -1 && !session) return agents;

  const replace = (next: AgentSummary): AgentSummary[] => {
    // A row built here may be the first copy of an agent the server has not
    // described yet, and a `current` row may predate normalisation; fill in the
    // fields serde omits so the list never carries a bare `undefined`.
    const row = normalizeAgentRecord(next);
    if (index === -1) return [row, ...agents];
    const list = [...agents];
    list[index] = row;
    return list;
  };

  switch (event.type) {
    case 'agent_spawned': {
      const base: AgentSummary = current ?? {
        id,
        session_id: event.session_id ?? '',
        parent_run_id: event.parent_run_id || null,
        parent_agent_id: null,
        name: event.name,
        role: event.role,
        task: event.task,
        status: 'running',
        model: event.model,
        iterations: 0,
        summary: '',
        error: null,
        owned_paths: [],
        created_at: event.ts_ms,
        updated_at: event.ts_ms,
        finished_at: null,
        background: event.background ?? false,
        note: '',
        progress: null,
        ...sessionContext(session),
      };
      return replace({
        ...base,
        name: event.name,
        role: event.role,
        task: event.task,
        model: event.model,
        status: 'running',
        // The row's own `background` copy comes from the store, but the live
        // frame is what a client sees first: without it the badge would only
        // appear after a refresh.
        background: event.background ?? base.background,
        updated_at: event.ts_ms,
        ...sessionContext(session),
      });
    }

    case 'agent_progress': {
      // A frame that beats `GET /api/agents` on a cold tab still proves the agent
      // exists, and carries the iteration count it has reached.
      const base =
        current ??
        placeholderAgent(id, event, session, {
          status: 'running',
          iterations: event.iterations,
        });
      return replace({
        ...base,
        iterations: event.iterations,
        updated_at: event.ts_ms,
      });
    }

    // A pause changes whether the agent can spend anything, and a note is what it
    // says about itself. Both are updates to the row the roster renders, so both
    // have to land in this reducer - not only in the agent's own activity feed.
    case 'agent_paused': {
      // Dropping this for an unknown row is what made a paused agent render
      // `running`: the frame arrived first, and the list read after it is the
      // last writer. The row is created paused instead, and the later list (which
      // carries the same status) confirms it.
      const base =
        current ??
        placeholderAgent(id, event, session, { status: event.paused ? 'paused' : 'running' });
      return replace({
        ...base,
        status: event.paused ? 'paused' : 'running',
        updated_at: event.ts_ms,
      });
    }

    case 'agent_note': {
      const base =
        current ??
        placeholderAgent(id, event, session, {
          status: 'running',
          note: event.note,
          progress: event.progress ?? null,
        });
      return replace({
        ...base,
        note: event.note,
        progress: event.progress ?? base.progress,
        updated_at: event.ts_ms,
      });
    }

    case 'agent_finished': {
      if (!current) {
        return replace({
          id,
          session_id: event.session_id ?? '',
          parent_run_id: null,
          parent_agent_id: null,
          name: 'subagent',
          role: 'subagent',
          task: '',
          status: event.status,
          model: '',
          iterations: 0,
          summary: event.summary,
          error: event.error ?? null,
          owned_paths: [],
          created_at: event.ts_ms,
          updated_at: event.ts_ms,
          finished_at: event.ts_ms,
          background: false,
          note: '',
          progress: null,
          ...sessionContext(session),
        });
      }
      return replace({
        ...current,
        status: event.status,
        summary: event.summary,
        error: event.error ?? null,
        updated_at: event.ts_ms,
        finished_at: event.ts_ms,
      });
    }

    default:
      return agents;
  }
}

/**
 * Merge agent records from a session snapshot into the workspace list.
 *
 * A record fetched with its session wins over a live-created placeholder (it has
 * the real workspace/title), and the list stays newest-first like the server's.
 */
export function mergeAgentList(
  current: AgentSummary[],
  incoming: AgentSummary[],
): AgentSummary[] {
  if (incoming.length === 0) return current;
  const byId = new Map<string, AgentSummary>();
  for (const agent of current) byId.set(agent.id, normalizeAgentRecord(agent));
  for (const agent of incoming) byId.set(agent.id, normalizeAgentRecord(agent));
  return [...byId.values()].sort((a, b) => b.created_at - a.created_at);
}

/** `tool_call` arguments are JSON text; a malformed payload stays as text. */
function toolArgs(raw: string | null | undefined): JsonValue {
  return parseArgs(raw ?? '');
}

/**
 * Build an activity feed from the messages already stored for one agent.
 *
 * The session snapshot contains every message (subagent rows included, marked
 * with `agent_id`), so a reload or a drill-in from another session can show the
 * history without a second request for the whole session.
 */
export function activityFromMessages(
  messages: StoredMessage[],
  agentId: string,
): AgentActivity {
  const items: AgentActivityItem[] = [];
  const calls = new Map<string, number>();

  const pushToolCall = (call: { id: string; name: string; arguments: string }, at: number, runId: string | null): void => {
    const args = toolArgs(call.arguments);
    if (call.name === 'run_command') {
      const item: CommandItem = {
        kind: 'command',
        id: `cmd:${call.id}`,
        at,
        callId: call.id,
        command: stringArg(args, 'command'),
        cwd: stringArg(args, 'cwd') || '.',
        stdout: '',
        stderr: '',
        output: '',
        exitCode: null,
        status: 'ok',
        truncated: false,
        durationMs: null,
        runId,
      };
      calls.set(call.id, items.length);
      items.push(item);
      return;
    }
    const item: ToolItem = {
      kind: 'tool',
      id: `tool:${call.id}`,
      at,
      callId: call.id,
      name: call.name,
      args,
      status: 'ok',
      output: '',
      truncated: false,
      durationMs: null,
      runId,
    };
    calls.set(call.id, items.length);
    items.push(item);
  };

  for (const message of messages) {
    if (message.agent_id !== agentId) continue;
    if (message.role === 'user') {
      items.push({
        kind: 'message',
        id: `msg:${message.id}`,
        at: message.created_at,
        role: 'user',
        content: message.content,
        reasoning: '',
        streaming: false,
        runId: message.run_id ?? null,
        agentId,
      });
    } else if (message.role === 'assistant') {
      if (message.content.trim() !== '') {
        items.push({
          kind: 'message',
          id: `msg:${message.id}`,
          at: message.created_at,
          role: 'assistant',
          content: message.content,
          reasoning: '',
          streaming: false,
          runId: message.run_id ?? null,
          agentId,
        });
      }
      for (const call of message.tool_calls ?? []) {
        pushToolCall(call, message.created_at, message.run_id ?? null);
      }
    } else if (message.role === 'tool') {
      const index = message.tool_call_id ? calls.get(message.tool_call_id) : undefined;
      const target = index === undefined ? undefined : items[index];
      if (!target) continue;
      if (target.kind === 'command') {
        const report = parseStoredCommandReport(message.content);
        if (report.hasSections || report.exitCode !== null || report.durationMs !== null) {
          target.stdout = report.stdout;
          target.stderr = report.stderr;
          target.exitCode = report.exitCode;
          target.durationMs = report.durationMs;
          target.truncated = report.truncated;
          target.status = report.ok ? 'ok' : 'error';
          target.output = '';
        } else {
          target.output = message.content;
        }
      } else if (target.kind === 'tool') {
        target.output = message.content;
      }
    }
  }

  return { items: trimItems(items), lastSeq: 0 };
}

/**
 * Combine a stored transcript with the live feed without duplicating rows.
 *
 * Items are keyed by their stable id (`msg:<row>` / `tool:<call_id>`), so a live
 * item wins over the stored copy of the same call and both are ordered by time.
 */
export function mergeActivities(base: AgentActivity, overlay: AgentActivity): AgentActivity {
  if (overlay.items.length === 0) return base;
  if (base.items.length === 0) return overlay;
  const byId = new Map<string, AgentActivityItem>();
  for (const item of base.items) byId.set(item.id, item);
  for (const item of overlay.items) byId.set(item.id, item);
  const items = [...byId.values()].sort((a, b) => a.at - b.at);
  return {
    items: trimItems(items),
    lastSeq: Math.max(base.lastSeq, overlay.lastSeq),
  };
}
