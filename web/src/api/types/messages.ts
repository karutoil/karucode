export interface ToolCall {
  id: string;
  name: string;
  arguments: string;
}

export type MessageRole = 'system' | 'user' | 'assistant' | 'tool';

export interface StoredMessage {
  id: number;
  session_id: string;
  run_id: string | null;
  agent_id: string | null;
  role: MessageRole;
  content: string;
  tool_calls: ToolCall[] | null;
  tool_call_id: string | null;
  name: string | null;
  /** Images the tool returned for a vision model (base64, no `data:` prefix). */
  images?: ImageAttachment[] | null;
  /**
   * The model's reasoning for this turn, when the endpoint streams analysis
   * separately from the answer (`reasoning_content`). The server stores it with
   * the assistant row, so a reloaded transcript can still show the thinking
   * block; absent on older rows and on models that do not think out loud.
   */
  reasoning_content?: string | null;
  /**
   * When set, a rewind hid this message. The server already filters hidden rows
   * out of every read, so this is only present on a row a caller fetched by id.
   */
  rewound_at?: number | null;
  created_at: number;
}

/**
 * An image a tool handed the model, exactly as the server stores it. `data` is
 * standard base64; the UI turns it into a data URL for `<img src>`. Only the
 * media types the vision APIs accept are ever produced (`image/png`, `jpeg`,
 * `gif`, `webp`).
 */
export interface ImageAttachment {
  media_type: string;
  data: string;
  bytes: number;
  name?: string | null;
}

/**
 * What `POST /api/sessions/:id/messages` answered.
 *
 * A message that is a slash command is answered in place (`mode: "command"`): no
 * run starts, and the result is written to the transcript as its own row. Every
 * other message starts a run, which is the historical shape and stays the
 * default so an older client keeps working unchanged.
 */
export type SendMessageResponse =
  | { mode?: 'send'; run_id: string; session_id: string }
  | {
      mode: 'command';
      session_id: string;
      /** The command without its slash. */
      command: string;
      /** The transcript row the answer was written to. */
      message_id: number;
      state: 'ok' | 'error' | string;
    };

// ---------------------------------------------------------------------------
// Queued messages (steer / follow-up / recall)
// ---------------------------------------------------------------------------

/** How a queued message relates to the run it was accepted during. */
export type QueuedKind = 'steer' | 'follow_up';

/**
 * One file attached to a queued message.
 *
 * Only the chip: the bytes are already inside the message's `content`, which is
 * what the model sees. `path` is where an upload landed (or the inline file's
 * own path).
 */
export interface QueuedAttachment {
  name: string;
  path: string;
  mode: 'inline' | 'upload';
  /** Bytes; `0` means the server did not record a size. */
  size: number;
  /**
   * The inlined text of an `inline` attachment.
   *
   * Sent back so a recall can rebuild the chip the composer drew: the browser
   * read this from a local file the server never saw, so it is the only copy
   * that can be inlined again. An upload has none - the file is on disk at
   * `path`.
   */
  content?: string;
}

/** One message waiting for a run to pick it up. */
export interface QueuedMessage {
  /** `steer_…` or `followup_…`; what a recall names. */
  id: string;
  kind: QueuedKind;
  /** The exact text the model will see (draft + one block per attachment). */
  content: string;
  /**
   * The draft to restore on recall; the content when nothing was attached.
   *
   * **Optional on the wire.** Rust declares it `Option<String>` with
   * `skip_serializing_if = "Option::is_none"` (`kc-core/src/queue.rs`), so it is
   * absent for the normal case - a message with no attachments, where the draft
   * and the content are the same string. `GET /api/sessions/:id/queues` hides
   * that behind `public_view()`, but the `queued_messages` SSE frame serialises
   * the raw struct, so a required field here was a lie that only survived because
   * every consumer happened to write `entry.draft || entry.content`.
   *
   * It is normalised once, at the state boundary (`queuedLists` in `chat.ts`), so
   * everything downstream can read it as a string. Making it a required field on
   * the type is the wrong fix: it would break the moment a frame omitted it, and
   * the *server* is what should change (see the follow-up note in `chat.ts`).
   */
  draft?: string;
  attachments?: QueuedAttachment[];
  accepted_at: number;
}

/** `GET /api/sessions/:id/queues` — both queues, without the transcript. */
export interface QueuesResponse {
  session_id: string;
  running: boolean;
  steers: QueuedMessage[];
  follow_ups: QueuedMessage[];
}

/** `POST /api/sessions/:id/messages?mode=steer|follow_up` → `202`. */
export interface QueuedAccepted {
  mode: QueuedKind;
  session_id: string;
  /** Present for `steer`. */
  steer_id?: string;
  /** Present for `follow_up`. */
  follow_up_id?: string;
  queued: true;
  /** The entry as it was queued, so the panel can draw it immediately. */
  queued_message?: QueuedMessage;
}

/** `DELETE /api/sessions/:id/queues/:id` — the message, handed back. */
export interface RecallResponse {
  session_id: string;
  recalled: QueuedMessage;
}

// ---------------------------------------------------------------------------
// Rewind
// ---------------------------------------------------------------------------

/**
 * Where a rewind cuts and what it hides.
 *
 * `keep_through` is the id of the last *visible* message. `before_prompt` is the
 * id in front of it, used by the retry and note modes, which hide the chosen
 * prompt as well so the replacement turn does not follow it directly.
 */
export interface RewindPoint {
  session_id: string;
  user_message_id: number;
  prompt: string;
  keep_through: number;
  before_prompt: number;
  messages_removed: number;
}

/** One file a rewind will touch. */
export interface RewindFile {
  path: string;
  /** The bytes restored, or `null` when the file is removed instead. */
  restore?: string | null;
  /** True when the rewind deletes the file (the agent created it). */
  removes_file: boolean;
  /** How many journal rows in the region changed this path. */
  changes: number;
  /** Tools that touched it, in the order they ran. */
  tools: string[];
  /** True when a before-image is missing, so the file cannot be fully restored. */
  incomplete: boolean;
}

/** What a rewind would do, computed by the server from the journal it will replay. */
export interface RewindPreview {
  session_id: string;
  message_id: number;
  messages_removed: number;
  files: RewindFile[];
  incomplete: boolean;
  /**
   * Tools the rewound turn called that cannot be undone — `run_command`, a
   * plugin, an MCP call. Their effects stay; the dialog says so rather than
   * implying a clean revert.
   */
  opaque_tools: string[];
}

/** `GET /api/sessions/:id/rewind?message=N`. Mutates nothing. */
export interface RewindPreviewResponse {
  session_id: string;
  point: RewindPoint;
  preview: RewindPreview;
  running: boolean;
}

export type RewindUndoOutcome = 'restored' | 'removed' | 'not_captured' | 'failed';

/** How a rewind relates to the turn it replaces. */
export type RewindMode = 'none' | 'retry' | 'note';

export interface RewindRequest {
  message_id: number;
  /** Undo the file changes made after the message. Off by default. */
  revert?: boolean;
  /**
   * `none` hides the tail and stops, `retry` re-sends the original prompt,
   * `note` sends `note` instead.
   */
  mode?: RewindMode;
  /** The replacement prompt for `mode: 'note'`. */
  note?: string;
  /** Whether the chosen prompt survives. Defaults per mode on the server. */
  keep_prompt?: boolean;
}

/** `POST /api/sessions/:id/rewind`. */
export interface RewindResponse {
  session_id: string;
  point: RewindPoint;
  preview: RewindPreview;
  /** One entry per journal row, newest first. */
  outcomes: RewindUndoOutcome[];
  /** The run started by `retry` or `note`; `null` for `none`. */
  run_id: string | null;
  mode: RewindMode;
  files_restored: number;
  files_skipped: number;
}
