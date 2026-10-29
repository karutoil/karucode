import type {
  AgentRecord,
  ApprovalDecision,
  ApprovalRequest,
  KarucodeEvent,
  KnownEvent,
  RunStatus,
  SessionSummary,
} from '../api/types';
import type {
  AgentView,
  ApprovalItem,
  ApprovalView,
  ChatItem,
  ChatState,
  CommandItem,
  ErrorItem,
  InterruptedItem,
  NoticeItem,
  ToolItem,
} from './chat';

const MAX_STREAM_CHARS = 200_000;

function clampOutput(text: string): { text: string; truncated: boolean } {
  if (text.length <= MAX_STREAM_CHARS) return { text, truncated: false };
  return { text: text.slice(0, MAX_STREAM_CHARS), truncated: true };
}

function closeStreaming(items: ChatItem[], keepId?: string): ChatItem[] {
  let changed = false;
  const next = items.map((item) => {
    if (item.kind === 'message' && item.streaming && item.id !== keepId) {
      changed = true;
      return { ...item, streaming: false };
    }
    return item;
  });
  return changed ? next : items;
}

function updateItem(
  state: ChatState,
  predicate: (item: ChatItem) => boolean,
  update: (item: ChatItem) => ChatItem,
): ChatState {
  let changed = false;
  const items = state.items.map((item) => {
    if (!predicate(item)) return item;
    changed = true;
    return update(item);
  });
  return changed ? { ...state, items } : state;
}

function findItem(state: ChatState, predicate: (item: ChatItem) => boolean): ChatItem | undefined {
  for (let i = state.items.length - 1; i >= 0; i -= 1) {
    const item = state.items[i];
    if (item && predicate(item)) return item;
  }
  return undefined;
}

function append(state: ChatState, item: ChatItem): ChatState {
  return { ...state, items: [...state.items, item], lastSeq: Math.max(state.lastSeq, 0) };
}

function upsert<T extends ChatItem>(state: ChatState, id: string, build: () => T): ChatState {
  if (state.items.some((item) => item.id === id)) return state;
  return append(state, build());
}

function appendNotice(state: ChatState, id: string, level: NoticeItem['level'], message: string, at: number): ChatState {
  return upsert(state, id, () => ({ kind: 'notice', id, at, level, message }));
}

function appendInterrupted(state: ChatState, runId: string, at: number): ChatState {
  const id = `interrupted:${runId}`;
  return upsert<InterruptedItem>(state, id, () => ({
    kind: 'interrupted',
    id,
    at,
    runId,
    message: 'This run was interrupted — the server restarted while it was working.',
  }));
}

function stopActiveWork(state: ChatState): ChatState {
  let changed = false;
  const items = state.items.map((item) => {
    if ((item.kind === 'tool' || item.kind === 'command') && item.status === 'running') {
      changed = true;
      return { ...item, status: 'stopped' as const };
    }
    return item;
  });
  const withoutEmpty = items.filter(
    (item): boolean =>
      !(item.kind === 'message' && item.role === 'assistant' && item.streaming && item.content === '' && item.reasoning === ''),
  );
  const next = closeStreaming(withoutEmpty);
  if (next === state.items && !changed) return state;
  return { ...state, items: next };
}

function normaliseLevel(level: string): NoticeItem['level'] {
  if (level === 'error' || level === 'danger') return 'error';
  if (level === 'warn' || level === 'warning') return 'warn';
  return 'info';
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

function agentFromEvent(
  event: Extract<KnownEvent, { type: 'agent_spawned' }>,
  sessionId: string,
): AgentView {
  return {
    id: event.agent_id,
    session_id: event.session_id ?? sessionId,
    parent_run_id: event.parent_run_id ?? null,
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
  };
}

function upsertAgent(state: ChatState, agentId: string, update: (agent: AgentView | null) => AgentView | null): ChatState {
  const index = state.agents.findIndex((agent) => agent.id === agentId);
  const current = index >= 0 ? state.agents[index] ?? null : null;
  const next = update(current);
  if (!next) return state;
  if (index === -1) return { ...state, agents: [...state.agents, next] };
  const agents = [...state.agents];
  agents[index] = next;
  return { ...state, agents };
}

/**
 * Pure reducer for the open session's transcript. Unknown event types, events
 * from other sessions and subagent text/tool events are ignored. Session list
 * updates live in `applyEventToSessions`.
 */
export function applyEventToChat(state: ChatState, event: KarucodeEvent): ChatState {
  if (state.sessionId === null) return state;
  if (event.session_id && event.session_id !== state.sessionId) return state;

  const sessionId = state.sessionId;
  const runId = event.run_id ?? state.runId;
  const at = event.ts_ms;
  const seen = { ...state, lastSeq: Math.max(state.lastSeq, event.seq) };

  switch (event.type) {
    case 'run_started': {
      let items = closeStreaming(state.items);
      const lastUser = findItem(
        state,
        (item) => item.kind === 'message' && item.role === 'user' && (item.runId === null || item.runId === event.run_id),
      );
      if (lastUser && lastUser.kind === 'message') {
        if (lastUser.runId !== event.run_id) {
          items = items.map((item) => (item.id === lastUser.id && item.kind === 'message' ? { ...item, runId: event.run_id } : item));
        }
      } else if (event.prompt) {
        items = [
          ...items,
          {
            kind: 'message',
            id: `user:${event.run_id}`,
            at,
            role: 'user',
            content: event.prompt,
            reasoning: '',
            streaming: false,
            runId: event.run_id,
            agentId: null,
          },
        ];
      }
      return {
        ...seen,
        items,
        running: true,
        runId: event.run_id,
        runStatus: 'running',
        runError: null,
        iteration: 0,
        usage: null,
      };
    }

    case 'text_delta': {
      if (event.agent_id) return state;
      if (!event.delta) return seen;
      const current = findItem(
        state,
        (item) => item.kind === 'message' && item.role === 'assistant' && item.streaming,
      );
      const tail = state.items[state.items.length - 1];
      if (current && current.kind === 'message' && tail === current) {
        return {
          ...seen,
          items: state.items.map((item) =>
            item.id === current.id && item.kind === 'message'
              ? { ...item, content: item.content + event.delta }
              : item,
          ),
        };
      }
      const items = closeStreaming(state.items);
      const id = `stream:${runId ?? 'run'}:${event.seq}:${items.length}`;
      return {
        ...seen,
        running: true,
        runStatus: state.running ? state.runStatus : 'running',
        items: [
          ...items,
          {
            kind: 'message',
            id,
            at,
            role: 'assistant',
            content: event.delta,
            reasoning: '',
            streaming: true,
            runId,
            agentId: null,
          },
        ],
      };
    }

    case 'reasoning_delta': {
      if (event.agent_id) return state;
      if (!event.delta) return seen;
      const current = findItem(
        state,
        (item) => item.kind === 'message' && item.role === 'assistant' && item.streaming,
      );
      const tail = state.items[state.items.length - 1];
      if (current && current.kind === 'message' && tail === current) {
        return {
          ...seen,
          items: state.items.map((item) =>
            item.id === current.id && item.kind === 'message'
              ? { ...item, reasoning: item.reasoning + event.delta }
              : item,
          ),
        };
      }
      const items = closeStreaming(state.items);
      return {
        ...seen,
        items: [
          ...items,
          {
            kind: 'message',
            id: `thinking:${runId ?? 'run'}:${event.seq}`,
            at,
            role: 'assistant',
            content: '',
            reasoning: event.delta,
            streaming: true,
            runId,
            agentId: null,
          },
        ],
      };
    }

    case 'stream_retry': {
      if (event.agent_id) return state;
      // The runtime threw the attempt that just died away: drop everything it
      // streamed (text and reasoning) so the retry is not shown twice, and say
      // why the answer restarted.
      const items = state.items.filter(
        (item) => !(item.kind === 'message' && item.streaming && item.runId === runId),
      );
      const kept = { ...seen, items: items.length === state.items.length ? state.items : items };
      return {
        ...appendNotice(
          kept,
          `stream-retry:${event.seq}`,
          'warn',
          `the model stream died mid-answer (${event.reason}); retrying (attempt ${
            event.attempt + 1
          } of ${event.max_attempts})`,
          at,
        ),
        running: true,
      };
    }

    case 'iteration_started':
      if (event.agent_id) return state;
      return { ...seen, iteration: event.iteration };

    case 'usage':
      if (event.agent_id) return state;
      return {
        ...seen,
        usage: {
          input_tokens: (state.usage?.input_tokens ?? 0) + event.input_tokens,
          output_tokens: (state.usage?.output_tokens ?? 0) + event.output_tokens,
        },
      };

    case 'tool_call_started': {
      if (event.agent_id) return state;
      const existing = state.items.find(
        (item) => (item.kind === 'tool' || item.kind === 'command') && item.callId === event.call_id,
      );
      if (existing) {
        const items = state.items.map((item) => {
          if (item.id !== existing.id) return item;
          if (item.kind === 'tool') return { ...item, name: event.name, args: event.arguments, status: 'running' as const };
          return item;
        });
        return { ...seen, items: closeStreaming(items), running: true };
      }
      const item: ToolItem = {
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
      };
      return { ...seen, items: [...closeStreaming(state.items), item], running: true };
    }

    case 'command_started': {
      if (event.agent_id) return state;
      const existing = state.items.find(
        (item) => (item.kind === 'tool' || item.kind === 'command') && item.callId === event.call_id,
      );
      if (existing) {
        const items = state.items.map((item) => {
          if (item.id !== existing.id) return item;
          if (item.kind === 'tool') {
            const command: CommandItem = {
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
            return command;
          }
          return { ...item, command: event.command, cwd: event.cwd, status: 'running' as const };
        });
        return { ...seen, items: closeStreaming(items), running: true };
      }
      const command: CommandItem = {
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
      };
      return { ...seen, items: [...closeStreaming(state.items), command], running: true };
    }

    case 'command_output': {
      if (event.agent_id) return state;
      return updateItem(
        { ...seen, items: state.items },
        (item) => item.kind === 'command' && item.callId === event.call_id,
        (item) => {
          const command = item as CommandItem;
          const target = event.stream === 'stderr' ? 'stderr' : 'stdout';
          const merged = clampOutput(command[target] + event.chunk);
          return {
            ...command,
            [target]: merged.text,
            truncated: command.truncated || merged.truncated,
          };
        },
      );
    }

    case 'command_finished': {
      if (event.agent_id) return state;
      return updateItem(
        { ...seen, items: state.items },
        (item) => item.kind === 'command' && item.callId === event.call_id,
        (item) => {
          const command = item as CommandItem;
          return {
            ...command,
            exitCode: event.exit_code,
            durationMs: event.duration_ms,
            truncated: command.truncated || event.truncated,
            status: event.exit_code === 0 ? 'ok' : 'error',
          };
        },
      );
    }

    case 'tool_call_finished': {
      if (event.agent_id) return state;
      const output = clampOutput(event.output).text;
      const target = state.items.find(
        (item) => (item.kind === 'tool' || item.kind === 'command') && item.callId === event.call_id,
      );
      if (!target) {
        const item: ToolItem = {
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
        };
        return { ...seen, items: [...state.items, item] };
      }
      return updateItem(
        { ...seen, items: state.items },
        (item) => item.id === target.id,
        (item) => {
          if (item.kind === 'command') {
            return {
              ...item,
              durationMs: event.duration_ms,
              truncated: item.truncated || event.truncated,
              status: event.ok ? item.status : ('error' as const),
              output: item.output === '' ? output : item.output,
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
      const existing = state.items.find((item) => item.kind === 'approval' && item.approval.id === view.id);
      if (existing) {
        return updateItem(state, (item) => item.id === existing.id && item.kind === 'approval', (item) => ({
          ...(item as ApprovalItem),
          approval: view,
        }));
      }
      const item: ApprovalItem = { kind: 'approval', id: `approval:${view.id}`, at, approval: view };
      return { ...seen, items: [...state.items, item] };
    }

    case 'approval_resolved': {
      const decision: ApprovalDecision = event.decision;
      return updateItem(
        seen,
        (item) => item.kind === 'approval' && item.approval.id === event.approval_id,
        (item) => {
          const approval = item as ApprovalItem;
          return {
            ...approval,
            approval: {
              ...approval.approval,
              status: decision === 'expired' ? 'expired' : 'resolved',
              decision,
            },
          };
        },
      );
    }

    case 'notice':
      return appendNotice(
        seen,
        `notice:${event.seq}`,
        normaliseLevel(event.level),
        event.message,
        at,
      );

    case 'stream_lagged':
      // The client refetches the session on this event, so the notice explains
      // the jump rather than asking for a manual reload.
      return appendNotice(
        seen,
        `lag:${event.seq}`,
        'warn',
        `The live stream fell behind and ${event.skipped} update(s) were dropped; the transcript was resynchronised from the server.`,
        at,
      );

    case 'error':
      return {
        ...seen,
        runError: { runId, message: event.message, kind: event.kind ?? null },
      };

    case 'run_finished': {
      const status: RunStatus = event.status;
      let next = stopActiveWork(seen);
      if (status === 'failed') {
        next = {
          ...next,
          runError: {
            runId: event.run_id,
            message: event.error ?? 'The run failed.',
            kind: null,
          },
        };
      } else if (status === 'cancelled') {
        next = appendNotice(next, `cancelled:${event.run_id}`, 'info', 'Run cancelled.', at);
      } else if (status === 'interrupted') {
        next = appendInterrupted(next, event.run_id, at);
      } else if (status === 'limit_reached') {
        next = appendNotice(
          next,
          `limit:${event.run_id}`,
          'warn',
          'The run stopped because it reached a configured limit.',
          at,
        );
      }
      return {
        ...next,
        running: false,
        runId: event.run_id,
        runStatus: status,
      };
    }

    case 'run_cancelled': {
      const next = stopActiveWork(seen);
      return {
        ...appendNotice(next, `cancelled:${event.run_id}`, 'info', 'Run cancelled.', at),
        running: false,
        runId: event.run_id,
        runStatus: 'cancelled',
      };
    }

    case 'agent_spawned': {
      return upsertAgent(seen, event.agent_id, (current) =>
        current
          ? {
              ...current,
              name: event.name,
              role: event.role,
              task: event.task,
              model: event.model,
              parent_run_id: event.parent_run_id ?? current.parent_run_id,
              status: 'running',
              updated_at: at,
            }
          : agentFromEvent(event, sessionId),
      );
    }

    case 'agent_progress':
      return upsertAgent(seen, event.agent_id, (current) =>
        current
          ? { ...current, iterations: event.iterations, updated_at: at }
          : null,
      );

    case 'agent_finished':
      return upsertAgent(seen, event.agent_id, (current) => {
        const base: AgentView = current ?? {
          id: event.agent_id,
          session_id: sessionId,
          parent_run_id: null,
          parent_agent_id: null,
          name: 'subagent',
          role: 'subagent',
          task: '',
          status: 'running',
          model: '',
          iterations: 0,
          summary: '',
          error: null,
          owned_paths: [],
          created_at: at,
          updated_at: at,
          finished_at: null,
        };
        return {
          ...base,
          status: event.status,
          summary: event.summary,
          error: event.error ?? null,
          updated_at: at,
          finished_at: at,
        };
      });

    case 'session_created':
    case 'session_renamed':
    case 'session_model_changed':
    case 'session_deleted':
      return seen;

    default:
      // Unknown event type: ignore gracefully.
      return state;
  }
}

function patchSession(
  sessions: SessionSummary[],
  sessionId: string,
  update: (session: SessionSummary) => SessionSummary,
): SessionSummary[] {
  let changed = false;
  const next = sessions.map((session) => {
    if (session.id !== sessionId) return session;
    changed = true;
    return update(session);
  });
  return changed ? next : sessions;
}

/** Pure reducer for the session rail. */
export function applyEventToSessions(sessions: SessionSummary[], event: KarucodeEvent): SessionSummary[] {
  switch (event.type) {
    case 'session_created': {
      if (sessions.some((session) => session.id === event.session_id)) {
        return patchSession(sessions, event.session_id, (session) => ({ ...session, title: event.title }));
      }
      return [
        ...sessions,
        {
          id: event.session_id,
          title: event.title,
          workspace: '',
          project_id: null,
          created_at: event.ts_ms,
          updated_at: event.ts_ms,
          message_count: 0,
          last_run_status: null,
          running: false,
        },
      ];
    }
    case 'session_renamed':
      return patchSession(sessions, event.session_id, (session) => ({
        ...session,
        title: event.title,
        updated_at: event.ts_ms,
      }));
    case 'session_model_changed':
      return patchSession(sessions, event.session_id, (session) => ({
        ...session,
        model: event.model,
        updated_at: event.ts_ms,
      }));
    case 'session_deleted':
      return sessions.filter((session) => session.id !== event.session_id);
    case 'run_started':
      if (!event.session_id) return sessions;
      return patchSession(sessions, event.session_id, (session) => ({
        ...session,
        running: true,
        last_run_status: 'running',
        message_count: session.message_count + 1,
        updated_at: event.ts_ms,
      }));
    case 'run_finished':
      if (!event.session_id) return sessions;
      return patchSession(sessions, event.session_id, (session) => ({
        ...session,
        running: false,
        last_run_status: event.status,
        message_count: session.message_count + 1,
        updated_at: event.ts_ms,
      }));
    case 'run_cancelled':
      if (!event.session_id) return sessions;
      return patchSession(sessions, event.session_id, (session) => ({
        ...session,
        running: false,
        last_run_status: 'cancelled',
        updated_at: event.ts_ms,
      }));
    default:
      return sessions;
  }
}

/** Exposed for tests and for the composer's optimistic pending state. */
export function markUserRun(items: ChatItem[], runId: string): ChatItem[] {
  let changed = false;
  const next = items.map((item) => {
    if (item.kind === 'message' && item.role === 'user' && item.runId === null) {
      changed = true;
      return { ...item, runId };
    }
    return item;
  });
  return changed ? next : items;
}

export function sessionAgentFromRecord(agent: AgentRecord): AgentView {
  return { ...agent, owned_paths: agent.owned_paths ?? [] };
}

/** Small helper used by tests and by seeded/live error banners. */
export function errorItem(runId: string | null, message: string, kind: string | null, at: number): ErrorItem {
  return { kind: 'error', id: `error:${runId ?? 'run'}:${at}`, at, message, errorKind: kind, runId };
}