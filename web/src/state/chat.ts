import type {
  AgentRecord,
  ApprovalDecision,
  ApprovalKind,
  ImageAttachment,
  JsonValue,
  QueuedMessage,
  RunRecord,
  RunStatus,
  SessionDetail,
  StoredMessage,
  ToolCall,
  Usage,
} from '../api/types';
import { preserveIterations } from '../lib/agentIterations';

export type ToolStatus = 'running' | 'ok' | 'error' | 'stopped';

export interface ApprovalView {
  id: string;
  kind: ApprovalKind;
  title: string;
  detail: string;
  payload: JsonValue;
  session_id: string;
  run_id: string;
  agent_id: string | null;
  status: 'pending' | 'resolved' | 'expired';
  decision: ApprovalDecision | null;
  created_at: number;
}

export interface MessageItem {
  kind: 'message';
  id: string;
  at: number;
  role: 'user' | 'assistant' | 'system';
  content: string;
  reasoning: string;
  streaming: boolean;
  runId: string | null;
  agentId: string | null;
  /**
   * The stored row behind this message, once it is known.
   *
   * `id` is the transcript's own key: `msg:12` for a row read from the server,
   * `user:<run>` or `stream:<run>:…` for one built from live events. A live event
   * carries no stored id, so the id arrives later (`withStoredMessageIds`) and
   * lives here rather than in `id` - the transcript keys its rows, heights and
   * entry animation on `id`, and rewriting it would re-mount the conversation.
   */
  messageId?: number;
}

export interface ToolItem {
  kind: 'tool';
  id: string;
  at: number;
  callId: string;
  name: string;
  args: JsonValue;
  status: ToolStatus;
  output: string;
  truncated: boolean;
  durationMs: number | null;
  runId: string | null;
  /** Images the tool attached for the model; the chat shows them as previews. */
  images?: ImageAttachment[];
  /**
   * The stored message this call belongs to (the assistant row that made it, or
   * the tool row that answered it), so the call can be a rewind point. Absent
   * while the call is live: a running call has no stored row yet.
   */
  messageId?: number;
}

export interface CommandItem {
  kind: 'command';
  id: string;
  at: number;
  callId: string;
  command: string;
  cwd: string;
  stdout: string;
  stderr: string;
  /** Fallback result text for commands restored from history. */
  output: string;
  exitCode: number | null;
  status: ToolStatus;
  truncated: boolean;
  durationMs: number | null;
  runId: string | null;
  /** The stored message behind this command; see {@link ToolItem.messageId}. */
  messageId?: number;
}

export interface ApprovalItem {
  kind: 'approval';
  id: string;
  at: number;
  approval: ApprovalView;
}

export interface ErrorItem {
  kind: 'error';
  id: string;
  at: number;
  message: string;
  errorKind: string | null;
  runId: string | null;
}

export interface NoticeItem {
  kind: 'notice';
  id: string;
  at: number;
  level: 'info' | 'warn' | 'error';
  message: string;
}

export interface InterruptedItem {
  kind: 'interrupted';
  id: string;
  at: number;
  runId: string;
  message: string;
}

/**
 * A slash command's answer, as it appears in the transcript.
 *
 * The server writes it as an assistant row named `slash:<command>` (or
 * `slash:<command>:error` for a refusal) and deliberately keeps it out of the
 * model's context: it is an answer for the person reading, not a turn in the
 * conversation. That is why this is its own item kind rather than a message -
 * drawn as the user's own action, with the command it came from on its face.
 */
export interface SlashItem {
  kind: 'slash';
  id: string;
  at: number;
  /** The command without its slash. */
  command: string;
  content: string;
  /** False when the command refused; the card says so instead of looking like an answer. */
  ok: boolean;
  messageId?: number;
}

export type ChatItem =
  | MessageItem
  | ToolItem
  | CommandItem
  | ApprovalItem
  | ErrorItem
  | NoticeItem
  | InterruptedItem
  | SlashItem;

/** The `name` prefix a slash command's result row carries (`kc_core::store`). */
export const SLASH_NAME_PREFIX = 'slash:';
/** Suffix a refused command's row carries after the command name. */
export const SLASH_ERROR_SUFFIX = ':error';

/**
 * Read a message's `name` as a slash-command result.
 *
 * `slash:compact` is an answer from `/compact`; `slash:compact:error` is a refusal.
 * Anything else - a tool row's own name, a plain assistant turn - is `null`.
 */
export function slashResultFrom(
  name: string | null | undefined,
): { command: string; ok: boolean } | null {
  if (typeof name !== 'string' || !name.startsWith(SLASH_NAME_PREFIX)) return null;
  const rest = name.slice(SLASH_NAME_PREFIX.length);
  const ok = !rest.endsWith(SLASH_ERROR_SUFFIX);
  const command = ok ? rest : rest.slice(0, rest.length - SLASH_ERROR_SUFFIX.length);
  if (command === '') return null;
  return { command, ok };
}

export type AgentView = AgentRecord;

/**
 * Fill in the `AgentRecord` fields the server is allowed to omit.
 *
 * `owned_paths` (`serde(default)`), `note` (`skip_serializing_if =
 * String::is_empty`) and `progress` (`skip_serializing_if = Option::is_none`)
 * all disappear from the JSON when they are empty/unset — on a live server, all
 * 77 rows of `GET /api/agents` carried neither `note` nor `progress`, running
 * ones included. Components read them as present, so every record is normalised
 * once, here, as it enters the state: a missing string becomes `''` and a
 * missing claim becomes `null`.
 *
 * The generic keeps the caller's more specific type (`AgentSummary` stays an
 * `AgentSummary`), so this is a drop-in for `{ ...agent, owned_paths: … }`.
 */
export function normalizeAgentRecord<T extends AgentRecord>(agent: T): T {
  return {
    ...agent,
    owned_paths: agent.owned_paths ?? [],
    note: agent.note ?? '',
    progress: agent.progress ?? null,
  };
}

export interface RunError {
  runId: string | null;
  message: string;
  kind: string | null;
}

export interface ChatState {
  sessionId: string | null;
  items: ChatItem[];
  agents: AgentView[];
  running: boolean;
  runId: string | null;
  runStatus: RunStatus | null;
  runError: RunError | null;
  usage: Usage | null;
  /**
   * Prompt tokens of the **newest** model call, which is how much of the context
   * window a single request is spending.
   *
   * This is deliberately not derived from `usage`: that one is accumulated across
   * every call for the cost HUD, and a sum answers "what has this session
   * cost", not "how full is the window". `null` until a call reports usage.
   */
  lastPromptTokens: number | null;
  iteration: number;
  lastSeq: number;
  /**
   * Messages the user sent while a run was working, oldest first.
   *
   * Not transcript rows: a steer joins the conversation only when the runtime
   * folds it in (and announces that with `steer_applied`), and a follow-up gets
   * its own run. Keeping them here is what lets the composer show "two messages
   * are waiting" and offer to take them back, including after a reload - the
   * snapshot carries the same pair.
   */
  queued: QueuedMessage[];
  /**
   * Queue ids this view has already seen leave the authoritative queue. The
   * bounded, session-local tombstones stop a slow HTTP 202 from re-adding a steer
   * or follow-up after its SSE frame says the runtime consumed it.
   */
  retiredQueuedIds: string[];
}

export function emptyChatState(sessionId: string | null = null): ChatState {
  return {
    sessionId,
    items: [],
    agents: [],
    running: false,
    runId: null,
    runStatus: null,
    runError: null,
    usage: null,
    lastPromptTokens: null,
    iteration: 0,
    lastSeq: 0,
    queued: [],
    retiredQueuedIds: [],
  };
}

/**
 * Split a session snapshot's two queues into the one list the composer shows.
 *
 * Steers come first because they land first: a steer is folded into the running
 * turn, while a follow-up waits for the run after it. Both keep their server-side
 * order, so "next to land" is the order the list is in.
 */
export function queuedFromDetail(detail: SessionDetail): QueuedMessage[] {
  return [...(detail.queued_steers ?? []), ...(detail.queued_follow_ups ?? [])];
}

/**
 * Replace the pending list from one source of truth.
 *
 * Used by the `queued_messages` event (which carries both queues in full) and by
 * the snapshot. Quietly drops entries with no id, because an entry the panel
 * cannot recall is worse than no row at all.
 *
 * This is also where a queued message is **normalised**. Rust omits `draft`
 * (`skip_serializing_if = "Option::is_none"`) whenever it equals `content`, and
 * the `queued_messages` SSE frame serialises the raw struct rather than
 * `public_view()`, so the field really can be missing. Filling it in here - the
 * one place both sources pass through - means every component downstream reads a
 * string and none of them has to remember `entry.draft || entry.content`.
 *
 * FOLLOW-UP FOR THE SERVER OWNER: the `queued_messages` event should carry
 * `public_view()` too. That is a crates/ change (the event variant holds
 * `Vec<QueuedMessage>`), so it is out of scope for the web client; until then this
 * boundary is what keeps the optional field honest.
 */
export function queuedLists(
  steers: readonly QueuedMessage[],
  followUps: readonly QueuedMessage[],
): QueuedMessage[] {
  return [...steers, ...followUps]
    .filter((entry) => typeof entry.id === 'string' && entry.id !== '')
    .map(normalizeQueuedMessage);
}

/**
 * Fill in the fields the wire is allowed to omit on one queued message.
 *
 * `draft` is the one that matters (see `queuedLists`); the generic keeps the
 * caller's type, so this is a drop-in wherever a queued message enters state.
 */
export function normalizeQueuedMessage<T extends QueuedMessage>(entry: T): T {
  return entry.draft === undefined ? { ...entry, draft: entry.content } : entry;
}

/** Tool-call arguments arrive as a JSON string; a parse failure is kept as text. */
export function parseArgs(raw: string): JsonValue {
  if (raw.trim() === '') return {};
  try {
    return JSON.parse(raw) as JsonValue;
  } catch {
    return raw;
  }
}

export interface ParsedCommandReport {
  stdout: string;
  stderr: string;
  exitCode: number | null;
  durationMs: number | null;
  truncated: boolean;
  hasSections: boolean;
  ok: boolean;
}

/**
 * Stored command results are a human-readable report (stdout/stderr are not
 * separate DB columns). Parse the stable header/section format so restored
 * command cards keep their exit code; fall back to the raw text if it does not
 * match.
 */
export function parseStoredCommandReport(content: string): ParsedCommandReport {
  const exitMatch = /^exit code:\s*(none|\d+)/m.exec(content);
  const durationMatch = /^duration:\s*([\d.]+)s/m.exec(content);
  const stdoutIndex = content.indexOf('--- stdout');
  const stderrIndex = content.indexOf('--- stderr');
  const hasSections = stdoutIndex !== -1;

  const sectionText = (start: number, end: number): string => {
    const headerEnd = content.indexOf('\n', start);
    if (headerEnd === -1) return '';
    const text = content.slice(headerEnd + 1, end === -1 ? content.length : end);
    const trimmed = text.replace(/\s+$/, '');
    return trimmed === '(empty)' ? '' : trimmed;
  };

  let stdout = '';
  let stderr = '';
  if (stdoutIndex !== -1) {
    stdout = sectionText(stdoutIndex, stderrIndex === -1 ? -1 : stderrIndex);
  }
  if (stderrIndex !== -1) stderr = sectionText(stderrIndex, -1);

  let exitCode: number | null = null;
  if (exitMatch?.[1] && exitMatch[1] !== 'none') {
    const parsed = Number(exitMatch[1]);
    exitCode = Number.isFinite(parsed) ? parsed : null;
  }

  let durationMs: number | null = null;
  if (durationMatch?.[1]) {
    const seconds = Number(durationMatch[1]);
    if (Number.isFinite(seconds)) durationMs = Math.round(seconds * 1000);
  }

  const terminatedBySignal = exitMatch?.[1] === 'none';
  return {
    stdout,
    stderr,
    exitCode,
    durationMs,
    truncated: /\btruncated\b/.test(content),
    hasSections,
    ok: !terminatedBySignal && (exitCode === null || exitCode === 0),
  };
}

export function stringArg(args: JsonValue, key: string): string {
  if (typeof args === 'object' && args !== null && !Array.isArray(args)) {
    const value = args[key];
    if (typeof value === 'string') return value;
  }
  return '';
}

/** One transcript range: the rows, the cursor older than them, and whether older rows exist. */
export interface TranscriptRows {
  messages: StoredMessage[];
  /** Exclusive cursor for the next older page; `null` when the start is loaded. */
  before: number | null;
  truncated: boolean;
}

/**
 * Merge a freshly fetched newest page into the rows the reader already loaded.
 *
 * A silent refresh (a reconnect, a stream resync, a run settling) used to replace
 * the store's rows with the server's newest page, so every page the reader had
 * scrolled back to was thrown away and "Load older history" came back. The
 * refreshed tail is always newer than the loaded range, so the union is simply
 * both ranges. Two exceptions stay with the fetched page:
 *
 * * the ranges do not touch (`overlap` false) — after a rewind, or when a session
 *   grew by more than one page while this tab was away. The union would jump
 *   over a hole the reader cannot see, so the fresh tail is the honest answer;
 * * the caller is forcing a reload (`openSession(..., { force: true })`), which
 *   is how rewind and recovery ask for exactly what the server now has.
 *
 * The cursor is derived from the union rather than copied from either response:
 * keeping a loaded older range while pointing the cursor above it would make the
 * next page overlap what is already on screen.
 */
export function mergeTranscriptRows(
  loaded: TranscriptRows | null,
  tail: { messages: readonly StoredMessage[]; messages_before?: number | null },
): TranscriptRows & { overlap: boolean } {
  const incoming = tail.messages;
  const incomingBefore = tail.messages_before ?? null;
  if (!loaded || loaded.messages.length === 0) {
    return { messages: [...incoming], before: incomingBefore, truncated: incomingBefore !== null, overlap: true };
  }
  if (incoming.length === 0) {
    return { ...loaded, overlap: true };
  }
  const byId = new Map<number, StoredMessage>();
  for (const message of loaded.messages) byId.set(message.id, message);
  for (const message of incoming) byId.set(message.id, message);
  const messages = [...byId.values()].sort((a, b) => a.id - b.id);
  // The two ranges overlap when the older set already reaches into the newer
  // one. A gap (oldest incoming newer than the newest loaded row) is a hole.
  let oldestIncoming = Number.POSITIVE_INFINITY;
  let newestIncoming = Number.NEGATIVE_INFINITY;
  for (const message of incoming) {
    if (message.id < oldestIncoming) oldestIncoming = message.id;
    if (message.id > newestIncoming) newestIncoming = message.id;
  }
  let oldestLoaded = Number.POSITIVE_INFINITY;
  let newestLoaded = Number.NEGATIVE_INFINITY;
  for (const message of loaded.messages) {
    if (message.id < oldestLoaded) oldestLoaded = message.id;
    if (message.id > newestLoaded) newestLoaded = message.id;
  }
  const overlap = newestLoaded >= oldestIncoming && newestIncoming >= oldestLoaded;
  if (!overlap) {
    return { messages: [...incoming], before: incomingBefore, truncated: incomingBefore !== null, overlap: false };
  }
  // Both cursors are "older than this id". The union is complete only when the
  // loaded range already reached the start; otherwise its oldest row is the next
  // boundary.
  const before =
    loaded.before === null
      ? null
      : incomingBefore === null
        ? null
        : Math.min(loaded.before, incomingBefore);
  return { messages, before, truncated: before !== null, overlap: true };
}

/**
 * Fill in the stored message id behind each row of a live transcript.
 *
 * A run's rows are built from events (`text_delta`, `tool_call_started`, …),
 * which carry run ids and call ids but **no stored message ids** - the ids only
 * exist once the row is written and the session is read back. That is why the
 * rewind control used to appear only after a reload: a live row could not name
 * the message a rewind would cut at, so every affordance refused to render.
 *
 * This walks the rows that are already on screen and attaches the id from the
 * snapshot, without touching item identity. Re-seeding the transcript instead
 * would rebuild every item id (`user:<run>` → `msg:<id>`), and the transcript
 * keys its rows, its measured heights and its entry animation on those ids: the
 * whole visible window would re-mount and fade at the end of every run.
 *
 * Matching is by facts the two sides share, never by position:
 * * a user/assistant prose row by its run and role - the *newest* stored row wins,
 *   because a turn that called tools wrote an assistant row per iteration and the
 *   prose the reader means is the last one;
 * * a call by its `call_id`, which is globally unique, so a tool card finds the
 *   row that made it (or the result row that answered it).
 *
 * Rows it cannot place are returned unchanged, which leaves them without a
 * rewind rather than pointing at the wrong message.
 */
export function withStoredMessageIds(items: ChatItem[], detail: SessionDetail): ChatItem[] {
  if (detail.messages.length === 0) return items;
  /** Stored prose rows by `run|role`, oldest first: a run can write several. */
  const rows = new Map<string, StoredMessage[]>();
  const byCallId = new Map<string, number>();
  for (const message of detail.messages) {
    if (message.agent_id) continue; // a subagent's rows are not this transcript's
    if (message.role === 'user' || message.role === 'assistant') {
      if (message.run_id) {
        const key = `${message.run_id}|${message.role}`;
        const list = rows.get(key);
        if (list) list.push(message);
        else rows.set(key, [message]);
      }
    }
    // The row that *made* the call places it in the turn the server will rewind
    // to. A result row is equally valid, so either side of the pair is a hit.
    for (const call of message.tool_calls ?? []) byCallId.set(call.id, message.id);
    if (message.tool_call_id && !byCallId.has(message.tool_call_id)) {
      byCallId.set(message.tool_call_id, message.id);
    }
  }

  /**
   * The stored row behind one prose item.
   *
   * A run writes one assistant row per iteration and one user row per steer, so
   * `run|role` alone can name several. The item's own text picks between them -
   * the live text of a settled row is exactly the stored content - and only when
   * no row agrees does the oldest free one stand in, which still rewinds to the
   * turn the reader clicked.
   *
   * Rows are claimed one-to-one, oldest first, in conversation order. Two
   * identical prompts in one run (a prompt and a steer that repeats it, "continue"
   * being the common one) otherwise both matched the *newest* row, so clicking the
   * first one sent the second one's id and rewound to the wrong turn. The
   * fallback is the *oldest* free row for the same reason: an item that matches
   * nothing (a `/skill` prompt, whose stored text is the expansion) must not take
   * the row a later item matches.
   *
   * A truncated window is a *tail*, so its candidate list can be missing the older
   * row an item belongs to. There the fallback is not taken at all, and an exact
   * match has to be unique: with two identical prompts and only the newer one
   * loaded, either id could rewind to the wrong turn - no button is better than
   * the wrong turn.
   */
  const claimed = new Set<number>();
  /**
   * A truncated detail is the *tail* of a session: the older row an item belongs to
   * may simply not be loaded. There only an unambiguous text can be trusted - one
   * that appears once among the loaded rows for its run and once among the live
   * items - because a text two items share is exactly the case where the loaded
   * row could belong to either, and a wrong turn is worse than no button.
   */
  const truncated = detail.messages_truncated === true;
  const sharedText = new Set<string>();
  if (truncated) {
    const seen = new Set<string>();
    for (const item of items) {
      if (item.kind !== 'message' || !item.runId || item.content === '') continue;
      const key = `${item.runId}|${item.role}|${item.content}`;
      if (seen.has(key)) sharedText.add(key);
      seen.add(key);
    }
  }
  const proseSource = (item: MessageItem): number | undefined => {
    if (!item.runId) return undefined;
    const candidates = rows.get(`${item.runId}|${item.role}`);
    if (candidates === undefined || candidates.length === 0) return undefined;
    const free = candidates.filter((candidate) => !claimed.has(candidate.id));
    if (truncated) {
      if (item.content === '') return undefined;
      if (sharedText.has(`${item.runId}|${item.role}|${item.content}`)) return undefined;
      const exact = free.filter((candidate) => candidate.content === item.content);
      if (exact.length !== 1) return undefined;
      claimed.add(exact[0]!.id);
      return exact[0]!.id;
    }
    if (item.content !== '') {
      // Oldest unclaimed match: the items are walked in conversation order, so the
      // n-th item with a given text takes the n-th row with that text.
      const match = free.find((candidate) => candidate.content === item.content);
      if (match !== undefined) {
        claimed.add(match.id);
        return match.id;
      }
    }
    const fallback = free[0];
    if (fallback !== undefined) claimed.add(fallback.id);
    return fallback?.id;
  };

  let changed = false;
  const next = items.map((item): ChatItem => {
    if (item.kind === 'message') {
      const id = proseSource(item);
      if (id === undefined || id === item.messageId) return item;
      changed = true;
      return { ...item, messageId: id };
    }
    if (item.kind === 'tool' || item.kind === 'command') {
      const id = byCallId.get(item.callId);
      if (id === undefined || id === item.messageId) return item;
      changed = true;
      return { ...item, messageId: id };
    }
    return item;
  });
  return changed ? next : items;
}

/** Builds the transcript for a freshly loaded session. */
export function seedChatState(detail: SessionDetail, previous?: ChatState): ChatState {
  const messages = [...detail.messages].sort((a, b) => a.created_at - b.created_at || a.id - b.id);
  const items: ChatItem[] = [];
  const callItems = new Map<string, ToolItem | CommandItem>();

  const pushToolCall = (call: ToolCall, message: StoredMessage): void => {
    const args = parseArgs(call.arguments);
    if (call.name === 'run_command') {
      const item: CommandItem = {
        kind: 'command',
        id: `cmd:${call.id}`,
        at: message.created_at,
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
        runId: message.run_id ?? null,
        messageId: message.id,
      };
      callItems.set(call.id, item);
      items.push(item);
    } else {
      const item: ToolItem = {
        kind: 'tool',
        id: `tool:${call.id}`,
        at: message.created_at,
        callId: call.id,
        name: call.name,
        args,
        status: 'ok',
        output: '',
        truncated: false,
        durationMs: null,
        runId: message.run_id ?? null,
        images: [],
        messageId: message.id,
      };
      callItems.set(call.id, item);
      items.push(item);
    }
  };

  for (const message of messages) {
    if (message.agent_id) continue; // subagent transcripts are shown in the Agents tab
    // A slash command's answer is a row of its own, tagged in `name`. It is drawn
    // as the action the user took, not as something the model said.
    const slash = slashResultFrom(message.name);
    if (slash) {
      items.push({
        kind: 'slash',
        id: `msg:${message.id}`,
        at: message.created_at,
        command: slash.command,
        content: message.content,
        ok: slash.ok,
        messageId: message.id,
      });
      continue;
    }
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
        agentId: message.agent_id ?? null,
      });
    } else if (message.role === 'assistant') {
      // The turn's reasoning is stored beside its prose, so a reloaded or
      // re-opened transcript shows the same thinking block that streamed live.
      // A turn that thought and then called a tool has no prose at all; the
      // reasoning is then the whole message, and dropping it would erase what
      // the model worked out before acting.
      const storedReasoning = message.reasoning_content ?? '';
      const reasoning = storedReasoning.trim() === '' ? '' : storedReasoning;
      if (message.content.trim() !== '' || reasoning !== '') {
        items.push({
          kind: 'message',
          id: `msg:${message.id}`,
          at: message.created_at,
          role: 'assistant',
          content: message.content,
          reasoning,
          streaming: false,
          runId: message.run_id ?? null,
          agentId: message.agent_id ?? null,
        });
      }
      for (const call of message.tool_calls ?? []) pushToolCall(call, message);
    } else if (message.role === 'tool') {
      const target = message.tool_call_id ? callItems.get(message.tool_call_id) : undefined;
      if (target) {
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
        } else {
          target.output = message.content;
          // The pixels are on the stored tool row, so a reloaded transcript still
          // shows exactly what the model was shown.
          target.images = message.images ?? [];
        }
      } else if (message.tool_call_id) {
        items.push({
          kind: 'tool',
          id: `tool:${message.tool_call_id}`,
          at: message.created_at,
          callId: message.tool_call_id,
          name: message.name ?? 'tool',
          args: {},
          status: 'ok',
          output: message.content,
          truncated: false,
          durationMs: null,
          runId: message.run_id ?? null,
          images: message.images ?? [],
          messageId: message.id,
        });
      }
    }
    // `system` messages are model context, not transcript.
  }

  const runs = [...detail.runs].sort((a, b) => a.started_at - b.started_at);
  const lastRun: RunRecord | undefined = runs[runs.length - 1];
  const lastRunId = lastRun?.id ?? null;
  const running = detail.running || lastRun?.status === 'running';
  let runError: RunError | null = null;
  if (lastRun?.status === 'failed') {
    runError = {
      runId: lastRun.id,
      message: lastRun.error ?? 'The run failed before producing a response.',
      kind: null,
    };
  }

  // Assistant turns are persisted when they *complete*, so a stored message for
  // this run is a finished turn: the answer still streaming belongs to a new item.
  // Reusing the stored one would glue the next deltas into finished prose, above
  // whatever tool cards follow it.
  let livePlaceholder: MessageItem | null = null;
  if (running && lastRun) {
    livePlaceholder = {
      kind: 'message',
      id: `stream:${lastRun.id}`,
      at: lastRun.started_at,
      role: 'assistant',
      content: '',
      reasoning: '',
      streaming: true,
      runId: lastRun.id,
      agentId: null,
    };
  }

  for (const approval of detail.approvals) {
    if (approval.status !== 'pending') continue;
    if (items.some((item) => item.kind === 'approval' && item.approval.id === approval.id)) continue;
    items.push({
      kind: 'approval',
      id: `approval:${approval.id}`,
      at: approval.created_at,
      approval: {
        id: approval.id,
        kind: approval.kind,
        title: approval.title,
        detail: approval.detail,
        payload: approval.payload,
        session_id: approval.session_id,
        run_id: approval.run_id,
        // `ApprovalRecord.decision` and `agent_id` are `skip_serializing_if` on the
        // server, so they arrive absent rather than null; `ApprovalView` keeps a
        // strict shape (`null`) so every consumer's `??`/truthiness test is
        // comparing one kind of "nothing".
        agent_id: approval.agent_id ?? null,
        status: approval.status,
        decision: approval.decision ?? null,
        created_at: approval.created_at,
      },
    });
  }

  const interruptedRuns = runs.filter((run) => run.status === 'interrupted').slice(-5);
  for (const run of interruptedRuns) {
    items.push({
      kind: 'interrupted',
      id: `interrupted:${run.id}`,
      at: run.finished_at ?? run.started_at,
      runId: run.id,
      message: 'This run was interrupted — the server restarted while it was working.',
    });
  }

  // A cancelled run keeps the prose and tool cards it produced before the stop,
  // and the live transcript says why they end there: one "Run cancelled." notice
  // per run. Nothing in the stored messages carries that statement, so a reload
  // rebuilt the partial turn with no word that it had been stopped. The same
  // notice, under the same id and in the same words - a run row is only settled
  // here, so a run still in flight never gets one.
  const cancelledRuns = runs.filter((run) => run.status === 'cancelled');
  for (const run of cancelledRuns) {
    items.push({
      kind: 'notice',
      id: `cancelled:${run.id}`,
      at: run.finished_at ?? run.started_at,
      level: 'info',
      message: 'Run cancelled.',
    });
  }

  // A run that stopped at a limit left no message of its own, so a reloaded
  // transcript used to show only the word "Limit reached" with no reason. The
  // run row carries the runtime's one-line explanation (the wall clock, a turn
  // ceiling, the model's output limit, or the loop guard); put it back as the
  // same notice the live run showed, under the same id, so the two do not stack.
  const limitRuns = runs
    .filter((run) => run.status === 'limit_reached' && (run.error ?? '').trim() !== '')
    .slice(-3);
  for (const run of limitRuns) {
    items.push({
      kind: 'notice',
      id: `limit:${run.id}`,
      at: run.finished_at ?? run.started_at,
      level: 'warn',
      message: run.error as string,
    });
  }

  // The snapshot cannot reproduce everything that is on screen: it only carries
  // *pending* approvals and nothing that was live-only. A refetch must not make a
  // card the user just approved (or a cancellation notice) disappear.
  const sameSession = previous !== undefined && previous.sessionId === detail.session.id;
  if (previous && sameSession) {
    const seen = new Set(items.map((item) => item.id));
    for (const item of previous.items) {
      if (seen.has(item.id)) continue;
      if (item.kind === 'approval' && item.approval.status !== 'pending') items.push(item);
      else if ((item.kind === 'notice' && item.id !== 'notice:history-truncated') || item.kind === 'interrupted') items.push(item);
    }
  }

  // Approvals and interruption markers are collected separately but belong where
  // they happened, not appended after the last message.
  const ordered = items
    .map((item, index) => ({ item, index }))
    .sort((a, b) => (a.item.at === b.item.at ? a.index - b.index : a.item.at - b.item.at))
    .map((entry) => entry.item);

  // The server returns the *newest* window of a long session. Say so at the head
  // of the transcript, otherwise it just starts mid-conversation with no
  // explanation of where the earlier history went.
  if (detail.messages_truncated) {
    ordered.unshift({
      kind: 'notice',
      id: 'notice:history-truncated',
      at: ordered[0]?.at ?? 0,
      level: 'info',
      message: 'Earlier history is not loaded.',
    });
  }

  if (livePlaceholder) ordered.push(livePlaceholder);

  // A refetch while a subagent is running returns the row as it was last
  // persisted, which can be older than the `agent_progress` frames already on
  // screen (and is `0` for an agent that has only just started). Keep the larger
  // count for the same session; a different session's agents are not carried over.
  const agents = preserveIterations(
    sameSession ? previous.agents : [],
    detail.agents.map(normalizeAgentRecord).sort((a, b) => a.created_at - b.created_at),
  );

  return {
    sessionId: detail.session.id,
    items: ordered,
    agents,
    running,
    runId: lastRunId,
    runStatus: lastRun?.status ?? null,
    runError,
    // Usage and iteration are state-only (never persisted), so a refetch of the
    // run that is still in flight must not reset the HUD mid-answer.
    usage: sameSession && previous.runId === lastRunId ? previous.usage : null,
    lastPromptTokens:
      sameSession && previous.runId === lastRunId ? previous.lastPromptTokens : null,
    iteration: sameSession && previous.runId === lastRunId ? previous.iteration : 0,
    lastSeq: 0,
    // The snapshot is authoritative for the queues: the runtime keeps them in
    // memory and reports them here, so this is what a reload (or a second tab)
    // sees. A live queue frame that arrived during the fetch was replayed by
    // `drainRefreshQueue` after this, so it wins only if it is newer.
    queued: queuedFromDetail(detail),
    retiredQueuedIds: sameSession ? previous.retiredQueuedIds : [],
  };
}