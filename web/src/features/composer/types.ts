import type { CommandSpec, QueuedAttachment, QueuedKind, QueuedMessage } from '../../api/types';
import type { Attachment } from '../../lib/attachments';
import type { TreeLoader } from '../../lib/mentions';
import type { Activity } from '../../state/selectors';

export interface UploadContext {
  session: string | null;
  project: string | null;
}

export interface ComposerFormState {
  draft: string;
  attachments: Attachment[];
  caret: number;
  sending: boolean;
  announcement: string;
}

export interface ScopedComposerForm {
  value: ComposerFormState;
  /** Present only while this session/project is the visible composer. */
  render?: () => void;
}

export type FieldUpdate<T> = T | ((current: T) => T);

export interface ComposerProps {
  disabledReason: string | null;
  running: boolean;
  stopping: boolean;
  status: Activity;
  /** Receives the final message text (draft + inlined/referenced attachments). */
  onSend: (text: string) => Promise<boolean>;
  /**
   * Send one slash command.
   *
   * Separate from `onSend` because a command is not a prompt: it is answered in
   * place, it never starts a run, and it is allowed while one is in flight (that
   * is when `/stop` matters). Defaults to `onSend`, which is correct for any
   * caller whose server answers commands through the same route.
   */
  onCommand?: (text: string) => Promise<boolean>;
  /**
   * Queue a message against the run that is already working.
   *
   * `mode` is what the toggle asked for: a steer joins the turn in flight, a
   * follow-up starts the run after it. The form is passed alongside the rendered
   * content so a recall can put both back.
   */
  onQueue?: (
    mode: QueuedKind,
    message: { content: string; draft: string; attachments: QueuedAttachment[] },
  ) => Promise<boolean>;
  /** Messages waiting for the run, newest last. */
  queued?: QueuedMessage[];
  /** Take a queued message back out; resolves `true` when it was recalled. */
  onRecall?: (queuedId: string) => Promise<boolean>;
  /** Queued ids whose recall is in flight, so the button can say so. */
  recalling?: Record<string, boolean>;
  onStop: () => void;
  onOpenSettings: () => void;
  /** Open Usage in this caller's workspace; omitted where no destination exists. */
  onOpenUsage?: () => void;
  focusTick: number;
  usageLabel?: string;
  /** Override the workspace tree fetch behind `@` suggestions (tests). */
  loadMentionTree?: TreeLoader;
  /** Override the command catalog fetch behind the `/` menu (tests). */
  loadCommands?: (project: string | null) => Promise<CommandSpec[]>;
  /** What `@` lists, for the hint line. The control chat mentions projects. */
  mentionLabel?: string;
  /** Override the attachment upload call (tests). */
  uploadFile?: (file: File, context: UploadContext) => Promise<{ path: string }>;
}
