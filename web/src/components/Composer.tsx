import type { JSX } from 'preact';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { Activity } from '../state/selectors';
import * as store from '../state/store';
import * as api from '../api/endpoints';
import { toErrorMessage } from '../api/client';
import type { QueuedAttachment, QueuedKind, QueuedMessage } from '../api/types';
import { describeApprovals } from '../lib/approvalCopy';
import {
  attachForQueue,
  attachmentSummary,
  attachmentsFromQueue,
  canSendAttachments,
  classifyFile,
  composeMessage,
  newAttachmentId,
  readBytes,
  readText,
  UPLOAD_MAX_BYTES,
  type Attachment,
} from '../lib/attachments';
import {
  activeMention,
  indexFromTree,
  insertMention,
  MENTION_INDEX_DEPTH,
  MENTION_LIMIT,
  rankMentions,
  type MentionCandidate,
  type TreeLoader,
} from '../lib/mentions';
import { formatBytes } from '../lib/format';
import { FileMentionMenu, mentionOptionId } from './FileMentionMenu';
import { ModelControls } from './ModelSwitcher';
import { PluginSlot } from './PluginSlot';
import { isUiHidden, UI_HIDE_COMPOSER_ATTACH, UI_SLOT_COMPOSER_TOOLBAR } from './uiContributions';
import {
  AlertIcon,
  ArrowUpIcon,
  CloseIcon,
  FileIcon,
  LockIcon,
  PaperclipIcon,
  PlusIcon,
  StopIcon,
} from './icons';
import {
  contextTone,
  contextUsage,
  formatContextPercent,
  type ContextUsage,
} from '../state/selectors';

export interface UploadContext {
  session: string | null;
  project: string | null;
}

interface ComposerFormState {
  draft: string;
  attachments: Attachment[];
  caret: number;
  sending: boolean;
  announcement: string;
}

interface ScopedComposerForm {
  value: ComposerFormState;
  /** Present only while this session/project is the visible composer. */
  render?: () => void;
}

type FieldUpdate<T> = T | ((current: T) => T);

/** One-line label for each queue kind, used by the toggle and the rows. */
const QUEUE_LABEL: Record<QueuedKind, string> = {
  steer: 'Steer',
  follow_up: 'Follow-up',
};

const QUEUE_TITLE: Record<QueuedKind, string> = {
  steer:
    'Steer: fold this into the turn that is already running. The model sees it at its next step, without losing the work in flight.',
  follow_up:
    'Follow-up: run this as a new turn once the current one finishes. The agent finishes what it is doing first.',
};

/**
 * Which queue a message typed while the agent works goes into.
 *
 * Shown only while a run is in flight, because that is the only time the choice
 * exists: with nothing running, sending *is* the follow-up. The two options are
 * genuinely different - a steer changes the turn in progress, a follow-up begins
 * the next one - so the toggle says which one is armed rather than hiding the
 * choice behind a modifier key.
 *
 * Keyboard behaviour is the ARIA radio-group contract, which a row of plain
 * buttons does not give you: exactly one option is in the tab order (the checked
 * one), Left/Up and Right/Down move *and* select the neighbour, Home/End jump to
 * the ends, and Space/Enter activate the focused option natively. Without the
 * roving tabindex a keyboard user had to tab through both options, and the arrow
 * keys did nothing, so the control announced itself as a radiogroup and then
 * refused to behave like one.
 */
function QueueModeToggle({
  mode,
  onChange,
}: {
  mode: QueuedKind;
  onChange: (next: QueuedKind) => void;
}): JSX.Element {
  const kinds = Object.keys(QUEUE_LABEL) as QueuedKind[];
  const buttons = useRef(new Map<QueuedKind, HTMLButtonElement | null>());

  /** Select an option and move focus to it, like a native radio button. */
  const choose = (next: QueuedKind): void => {
    onChange(next);
    buttons.current.get(next)?.focus();
  };

  const onKeyDown = (event: JSX.TargetedKeyboardEvent<HTMLButtonElement>): void => {
    const index = kinds.indexOf(mode);
    let next: QueuedKind | undefined;
    if (event.key === 'ArrowRight' || event.key === 'ArrowDown') {
      next = kinds[(index + 1) % kinds.length];
    } else if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') {
      next = kinds[(index - 1 + kinds.length) % kinds.length];
    } else if (event.key === 'Home') {
      next = kinds[0];
    } else if (event.key === 'End') {
      next = kinds[kinds.length - 1];
    }
    if (next === undefined) return;
    // Both keys would scroll the transcript otherwise.
    event.preventDefault();
    choose(next);
  };

  return (
    <div class="queue-mode" role="radiogroup" aria-label="What this message does">
      {kinds.map((kind) => (
        <button
          key={kind}
          ref={(element) => {
            buttons.current.set(kind, element);
          }}
          type="button"
          role="radio"
          aria-checked={mode === kind}
          // The checked option is the group's one tab stop; arrows move within.
          tabIndex={mode === kind ? 0 : -1}
          class="queue-mode-option"
          data-active={mode === kind ? 'true' : 'false'}
          title={QUEUE_TITLE[kind]}
          onClick={() => onChange(kind)}
          onKeyDown={onKeyDown}
        >
          {QUEUE_LABEL[kind]}
        </button>
      ))}
    </div>
  );
}

/**
 * The messages waiting for this run, with the one action that matters on each:
 * take it back.
 *
 * A queued message is invisible in the transcript by design (a steer only becomes
 * a turn when the loop folds it in), which made a mis-sent message impossible to
 * undo before this panel existed. Recall is honest about the race: the server
 * answers `404` if the run already applied the message, and the row says the same
 * thing rather than pretending the click always works.
 */
function QueuedPanel({
  queued,
  recalling,
  onRecall,
}: {
  queued: readonly QueuedMessage[];
  recalling: Record<string, boolean>;
  onRecall: (queuedId: string) => void;
}): JSX.Element | null {
  if (queued.length === 0) return null;
  return (
    <section class="queued-panel" aria-label="Queued messages">
      <p class="queued-head">
        {queued.length === 1 ? '1 message waiting' : `${queued.length} messages waiting`}
        <span class="queued-head-hint"> · the agent sees them in this order</span>
      </p>
      <ul class="queued-list">
        {queued.map((entry) => (
          <li key={entry.id} class="queued-row" data-kind={entry.kind}>
            <span class="queued-kind" title={QUEUE_TITLE[entry.kind]}>
              {QUEUE_LABEL[entry.kind]}
            </span>
            <span class="queued-body">
              <span class="queued-text" title={entry.content}>
                {entry.draft ?? entry.content}
              </span>
              {entry.attachments && entry.attachments.length > 0 ? (
                <span class="queued-attachments">
                  {entry.attachments.map((attachment) => (
                    <span
                      key={`${entry.id}:${attachment.path}`}
                      class="queued-attachment"
                      title={attachment.path}
                    >
                      <FileIcon size={10} />
                      {attachment.name}
                    </span>
                  ))}
                </span>
              ) : null}
            </span>
            <button
              type="button"
              class="queued-recall"
              disabled={recalling[entry.id] === true}
              onClick={() => onRecall(entry.id)}
              aria-label={`Recall ${QUEUE_LABEL[entry.kind].toLowerCase()}: ${entry.draft ?? entry.content}`}
              title="Take this back, with its attachments, and put it in the composer"
            >
              {recalling[entry.id] ? 'Recalling…' : 'Recall'}
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

/**
 * The provider-reported `57.4%` context-window reading is also the Usage entry
 * point when the caller has a Usage destination. Without one, the same telemetry
 * stays read-only; the composer never estimates usage from the unsubmitted draft.
 */
function ContextPill({
  usage,
  onOpenUsage,
}: {
  usage: ContextUsage;
  onOpenUsage?: () => void;
}): JSX.Element {
  const text = formatContextPercent(usage.fraction);
  const tone = contextTone(usage.fraction);
  const title =
    `${usage.tokens.toLocaleString()} of ${usage.window.toLocaleString()} context tokens used (${text}) — ` +
    'the prompt size the provider reported for the newest call. ' +
    'karucode compacts the oldest turns when the request passes this model\'s window. ' +
    'Open Usage for session details.';
  const ariaLabel =
    `Usage: context window ${text} full, ${usage.tokens.toLocaleString()} of ` +
    `${usage.window.toLocaleString()} tokens. Open session usage`;
  const content = (
    <>
      <span class="context-pill-label">Usage</span>
      <span class="context-pill-meter" aria-hidden="true">
        <span class="context-pill-fill" style={{ width: `${Math.max(2, usage.fraction * 100)}%` }} />
      </span>
      <span class="context-pill-text">{text}</span>
    </>
  );

  if (!onOpenUsage) {
    return (
      <span class="context-pill" data-tone={tone} title={title} aria-label={ariaLabel}>
        {content}
      </span>
    );
  }
  return (
    <button
      type="button"
      class="context-pill"
      data-tone={tone}
      title={title}
      aria-label={ariaLabel}
      onClick={() => onOpenUsage()}
    >
      {content}
    </button>
  );
}

export interface ComposerProps {
  disabledReason: string | null;
  running: boolean;
  stopping: boolean;
  status: Activity;
  /** Receives the final message text (draft + inlined/referenced attachments). */
  onSend: (text: string) => Promise<boolean>;
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
  /** What `@` lists, for the hint line. The control chat mentions projects. */
  mentionLabel?: string;
  /** Override the attachment upload call (tests). */
  uploadFile?: (file: File, context: UploadContext) => Promise<{ path: string }>;
}

const CHAR_COUNTER_THRESHOLD = 4000;
/** The mock's `.compose-area{max-height:150px}` now lives in CSS as
 * `--compose-input-max` on `.composer-input` (composer.css), which is what
 * actually clamps the box: an inline `height` cannot beat `max-height`, and a
 * viewport-dependent cap has to be a style rule rather than a constant. */
/** Suggestion index lifetime; the workspace changes under us all the time. */
const INDEX_TTL_MS = 60_000;
const MENTION_MENU_ID = 'composer-mention-menu';

const defaultLoadTree: TreeLoader = (depth = MENTION_INDEX_DEPTH) =>
  api.getWorkspaceTree('.', depth, store.currentProjectId.value);

const defaultUploadFile = async (
  file: File,
  context: UploadContext,
): Promise<{ path: string }> => {
  if (!context.session) throw new Error('Open a session before attaching files.');
  // The wire stays `application/octet-stream` per the contract; the response's
  // `path` is what the message references (the name may be deduplicated).
  const result = await api.uploadAttachment(
    { name: file.name, data: await readBytes(file) },
    { session: context.session, project: context.project },
  );
  return { path: result.path };
};

/**
 * Message composer: restyled to the mock (attach button, bottom action row,
 * footnote) with three behaviours layered on top —
 *
 * - `@` autocomplete over the real workspace index;
 * - local file attachments: small text is inlined, the rest is uploaded and
 *   referenced by workspace path;
 * - drag-and-drop and paste-to-attach, both landing in the same chip list.
 */
export function Composer({
  disabledReason,
  running,
  stopping,
  status,
  onSend,
  onQueue,
  queued,
  onRecall,
  recalling,
  onStop,
  onOpenSettings,
  onOpenUsage,
  focusTick,
  usageLabel,
  loadMentionTree,
  mentionLabel,
  uploadFile,
}: ComposerProps): JSX.Element {
  // One record per project/session: a different session in the same project must
  // not inherit a draft, just as the same session must not follow a later project.
  // Async upload/send continuations retain this record, not the next visible one.
  const sessionId = store.currentSessionId.value;
  const projectId = store.currentProjectId.value;
  const scopeKey = JSON.stringify([projectId, sessionId]);
  const forms = useRef(new Map<string, ScopedComposerForm>());
  let scopedForm = forms.current.get(scopeKey);
  if (!scopedForm) {
    scopedForm = {
      value: { draft: '', attachments: [], caret: 0, sending: false, announcement: '' },
    };
    forms.current.set(scopeKey, scopedForm);
  }
  const form = scopedForm;
  const { draft, attachments, caret, sending, announcement } = form.value;
  const [, renderForm] = useState(0);

  const updateForm = (update: (current: ComposerFormState) => ComposerFormState): void => {
    form.value = update(form.value);
    // When this scope is parked there is deliberately no subscriber to update.
    form.render?.();
  };
  const setDraft = (update: FieldUpdate<string>): void => {
    updateForm((current) => ({
      ...current,
      draft: typeof update === 'function' ? update(current.draft) : update,
    }));
  };
  const setAttachments = (update: FieldUpdate<Attachment[]>): void => {
    updateForm((current) => ({
      ...current,
      attachments: typeof update === 'function' ? update(current.attachments) : update,
    }));
  };
  const setAnnouncement = (update: FieldUpdate<string>): void => {
    updateForm((current) => ({
      ...current,
      announcement: typeof update === 'function' ? update(current.announcement) : update,
    }));
  };
  const setCaret = (update: FieldUpdate<number>): void => {
    updateForm((current) => ({
      ...current,
      caret: typeof update === 'function' ? update(current.caret) : update,
    }));
  };
  const setSending = (update: FieldUpdate<boolean>): void => {
    updateForm((current) => ({
      ...current,
      sending: typeof update === 'function' ? update(current.sending) : update,
    }));
  };

  useLayoutEffect(() => {
    const render = (): void => renderForm((current) => current + 1);
    form.render = render;
    return () => {
      if (form.render === render) form.render = undefined;
    };
  }, [form]);

  // `store.composerDraft` is how another surface (the empty state's quick
  // prompts) hands text to the composer; it is consumed once and cleared.
  // Reading `.value` here subscribes the composer to the signal, so a hand-off
  // that arrives while the composer is already mounted still lands (the empty
  // state and the composer are on screen together).
  const incomingDraft = store.composerDraft.value;
  useEffect(() => {
    if (incomingDraft === null) return;
    setDraft(incomingDraft);
    store.composerDraft.value = null;
    // Focus synchronously so the caret is already in the box when the click
    // returns; re-place it on the next frame, once the controlled value has
    // been committed to the DOM.
    const node = textarea.current;
    node?.focus();
    node?.setSelectionRange(incomingDraft.length, incomingDraft.length);
    requestAnimationFrame(() => {
      const element = textarea.current;
      if (!element) return;
      element.focus();
      element.setSelectionRange(incomingDraft.length, incomingDraft.length);
    });
  }, [incomingDraft]);

  // A recalled message comes in the same way, but restores the *form*: the draft
  // is appended to whatever has been typed since (a recall must never silently
  // replace a fresh draft), and the chips come back so sending again produces the
  // same request the recall took back.
  const handoff = store.composerHandoff.value;
  useEffect(() => {
    if (handoff === null) return;
    store.composerHandoff.value = null;
    setDraft((current) => {
      if (current.trim() === '') return handoff.text;
      return `${current.replace(/\s+$/, '')}\n\n${handoff.text}`;
    });
    setAttachments((current) => {
      if (handoff.attachments.length === 0) return current;
      // Two recalls of the same file must not collapse into one chip: the ids are
      // regenerated per recall, and `attachmentsFromQueue` already does that.
      return [...current, ...attachmentsFromQueue(handoff.attachments)];
    });
    setAnnouncement(
      handoff.attachments.length > 0
        ? `Recalled to the composer with ${handoff.attachments.length} ${
            handoff.attachments.length === 1 ? 'attachment' : 'attachments'
          }.`
        : 'Recalled to the composer.',
    );
    requestAnimationFrame(() => {
      const element = textarea.current;
      if (!element) return;
      element.focus();
      element.setSelectionRange(element.value.length, element.value.length);
    });
  }, [handoff]);
  const [dragging, setDragging] = useState(false);
  /**
   * Which queue the next message goes into while a run is in flight.
   *
   * `steer` is the default because it is the more useful of the two mid-run ("also
   * check the fixtures"), and a follow-up is the rarer intent: with nothing
   * running the toggle is hidden and this value is unused, because sending *is*
   * the follow-up then.
   */
  const [queueMode, setQueueMode] = useState<QueuedKind>('steer');
  const textarea = useRef<HTMLTextAreaElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const dragDepth = useRef(0);

  // ---- @-mentions -------------------------------------------------------
  const [suggestions, setSuggestions] = useState<MentionCandidate[]>([]);
  const [activeOption, setActiveOption] = useState(0);
  const [indexError, setIndexError] = useState<string | null>(null);
  const [indexLoading, setIndexLoading] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const indexedAt = useRef(0);
  const loadTree = loadMentionTree ?? defaultLoadTree;

  // The index is per project. The TTL alone would keep the previous project's
  // file list for up to a minute after a switch, and a mention could then name a
  // path the agent cannot open.
  const indexedProject = useRef(projectId);
  useEffect(() => {
    if (indexedProject.current === projectId) return;
    indexedProject.current = projectId;
    indexedAt.current = 0;
    setSuggestions([]);
  }, [projectId]);

  const mention = activeMention(draft, caret);
  const mentionOpen = mention !== null && !dismissed;
  const ranked = mentionOpen && mention ? rankMentions(suggestions, mention.query, MENTION_LIMIT) : [];
  const activeIndex = ranked.length === 0 ? 0 : Math.min(activeOption, ranked.length - 1);
  const menuOpen = mentionOpen && (ranked.length > 0 || indexLoading || indexError !== null);

  useEffect(() => {
    if (!mentionOpen) return;
    const fresh = indexedAt.current > 0 && Date.now() - indexedAt.current < INDEX_TTL_MS;
    if (fresh) {
      setIndexLoading(false);
      return;
    }
    let cancelled = false;
    const requestedProject = projectId;
    setIndexLoading(true);
    setIndexError(null);
    void (async () => {
      try {
        const tree = await loadTree(MENTION_INDEX_DEPTH);
        // A response can outlive both this popover and a project switch. Never
        // let the previous workspace populate the next project's menu.
        if (cancelled || store.currentProjectId.value !== requestedProject) return;
        setSuggestions(indexFromTree(tree).filter((entry) => entry.kind === 'file'));
        indexedAt.current = Date.now();
      } catch (error) {
        if (cancelled || store.currentProjectId.value !== requestedProject) return;
        setIndexError(`Could not index the workspace: ${toErrorMessage(error)}`);
      } finally {
        if (!cancelled && store.currentProjectId.value === requestedProject) {
          setIndexLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [mentionOpen, loadTree, projectId]);

  // A new query always starts at the best match.
  useEffect(() => {
    setActiveOption(0);
  }, [mention?.query, mentionOpen]);

  const chooseMention = (candidate: MentionCandidate): void => {
    if (!mention) return;
    const next = insertMention(draft, mention, candidate.path);
    setDraft(next.text);
    setCaret(next.caret);
    setDismissed(false);
    setActiveOption(0);
    // The textarea is controlled: move the caret once the value has rendered.
    window.setTimeout(() => {
      const element = textarea.current;
      if (!element) return;
      element.focus();
      element.setSelectionRange(next.caret, next.caret);
    }, 0);
  };

  // ---- attachments ------------------------------------------------------
  const updateAttachment = (id: string, patch: Partial<Attachment>): void => {
    setAttachments((current) =>
      current.map((attachment) => (attachment.id === id ? { ...attachment, ...patch } : attachment)),
    );
  };

  const processFile = async (
    attachment: Attachment,
    file: File,
    upload: typeof defaultUploadFile,
  ): Promise<void> => {
    if (attachment.mode === 'inline') {
      try {
        const content = await readText(file);
        updateAttachment(attachment.id, { status: 'ready', content });
      } catch {
        updateAttachment(attachment.id, { status: 'error', error: 'Could not read the file' });
      }
      return;
    }
    try {
      // The ids are captured before classification, file reads and the upload.
      // Reading them after an await can upload A's file into B's session.
      const result = await upload(file, {
        session: sessionId,
        project: projectId,
      });
      updateAttachment(attachment.id, { status: 'ready', path: result.path });
      // The uploaded file is now mentionable in its own workspace. A completion
      // that arrives after a project switch must not seed this project's index.
      if (store.currentProjectId.value === projectId) {
        setSuggestions((current) =>
          current.some((entry) => entry.path === result.path)
            ? current
            : [...current, { path: result.path, kind: 'file', size: attachment.size }],
        );
      }
    } catch (error) {
      updateAttachment(attachment.id, { status: 'error', error: toErrorMessage(error) });
    }
  };

  const addFiles = async (files: readonly File[]): Promise<void> => {
    if (files.length === 0) return;
    const accepted: { attachment: Attachment; file: File }[] = [];
    for (const file of files) {
      const decision =
        file.size > UPLOAD_MAX_BYTES
          ? { mode: 'upload' as const, reason: `Larger than ${formatBytes(UPLOAD_MAX_BYTES)}` }
          : await classifyFile(file).catch(() => ({ mode: 'upload' as const, reason: 'Could not read the file' }));
      const attachment: Attachment = {
        id: newAttachmentId(file.name),
        name: file.name,
        size: file.size,
        mode: decision.mode,
        path: file.name,
        status: file.size > UPLOAD_MAX_BYTES ? 'error' : decision.mode === 'inline' ? 'reading' : 'uploading',
        ...(file.size > UPLOAD_MAX_BYTES ? { error: decision.reason } : {}),
      };
      accepted.push({ attachment, file });
    }
    setAttachments((current) => [...current, ...accepted.map((entry) => entry.attachment)]);
    setAnnouncement(
      accepted.length === 1
        ? `Attached ${accepted[0]?.attachment.name ?? 'file'}.`
        : `Attached ${accepted.length} files.`,
    );
    await Promise.all(
      accepted
        .filter((entry) => entry.attachment.status !== 'error')
        .map((entry) => processFile(entry.attachment, entry.file, uploadFile ?? defaultUploadFile)),
    );
  };

  const dropAttachment = (id: string): void => {
    const removed = attachments.find((attachment) => attachment.id === id);
    setAttachments((current) => current.filter((attachment) => attachment.id !== id));
    if (removed) setAnnouncement(`Removed ${removed.name}.`);
  };

  const resetDrag = (): void => {
    dragDepth.current = 0;
    setDragging(false);
  };

  const hasFiles = (event: DragEvent): boolean => {
    const types = event.dataTransfer?.types;
    if (!types) return false;
    return Array.from(types).includes('Files');
  };

  // ---- layout effects ---------------------------------------------------
  const growTextarea = useCallback((): void => {
    const element = textarea.current;
    if (!element) return;
    // Grow to fit the draft and let CSS clamp it: `max-height` is
    // `var(--compose-input-max)`, and `max-height` wins over an inline `height`.
    // Re-measuring on resize also matters: the same text wraps onto more lines
    // on a phone, and retaining a desktop-height inline value would clip it.
    element.style.height = 'auto';
    element.style.height = `${element.scrollHeight}px`;
  }, []);

  useEffect(() => {
    growTextarea();
  }, [draft, growTextarea]);

  useEffect(() => {
    window.addEventListener('resize', growTextarea);
    return () => window.removeEventListener('resize', growTextarea);
  }, [growTextarea]);

  useEffect(() => {
    if (focusTick === 0) return;
    const narrow =
      typeof window !== 'undefined' && typeof window.matchMedia === 'function'
        ? window.matchMedia('(max-width: 1023px)').matches
        : false;
    if (narrow) return;
    const timer = window.setTimeout(() => textarea.current?.focus(), 30);
    return () => window.clearTimeout(timer);
  }, [focusTick]);

  const canType = disabledReason === null;
  const attachmentReady = canSendAttachments(attachments);
  const hasContent = draft.trim() !== '' || attachments.some((attachment) => attachment.status === 'ready');
  // While a run is in flight the same box sends into the queue instead of
  // starting one, because starting one is exactly what the API refuses with a
  // 409. `canQueue` is therefore the "running" half of `canSend`.
  const canQueue = canType && running && !sending && attachmentReady && hasContent && onQueue !== undefined;
  const canSend = canType && !running && !sending && attachmentReady && hasContent;

  const clearComposer = (
    message: string,
    submitted?: { draft: string; attachments: Attachment[] },
  ): void => {
    // Sending is asynchronous and the textarea stays editable. If the reader
    // changed this scoped form while the request was pending, preserve that work.
    if (
      submitted &&
      (form.value.draft !== submitted.draft || form.value.attachments !== submitted.attachments)
    ) {
      setAnnouncement(message);
      return;
    }
    setDraft('');
    setCaret(0);
    setAttachments([]);
    setDismissed(true);
    setAnnouncement(message);
  };

  const submit = async (): Promise<void> => {
    // While the agent works, the same box queues: a steer joins the turn in
    // flight, a follow-up starts the next one.
    if (running) {
      if (!canQueue) return;
      const submitted = { draft, attachments };
      setSending(true);
      try {
        const accepted = await onQueue(queueMode, {
          content: composeMessage(draft, attachments),
          draft,
          attachments: attachForQueue(attachments),
        });
        if (accepted) {
          const count = attachments.length;
          const what = QUEUE_LABEL[queueMode].toLowerCase();
          clearComposer(
            count === 0
              ? `Queued as a ${what}.`
              : `Queued as a ${what} with ${count} ${count === 1 ? 'attachment' : 'attachments'}.`,
            submitted,
          );
        }
      } finally {
        setSending(false);
      }
      return;
    }
    if (!canSend) return;
    const submitted = { draft, attachments };
    setSending(true);
    try {
      const accepted = await onSend(composeMessage(draft, attachments));
      if (accepted) {
        const sent = attachments.length;
        clearComposer(
          sent === 0 ? '' : `Sent with ${sent} ${sent === 1 ? 'attachment' : 'attachments'}.`,
          submitted,
        );
      }
    } finally {
      setSending(false);
    }
  };

  const syncCaret = (element: HTMLTextAreaElement): void => {
    setCaret(element.selectionStart ?? element.value.length);
  };

  // Phones get a short placeholder: at 16px (the iOS no-zoom size) the desktop
  // sentence wraps to two lines in a 390px column, and an empty dock would then
  // stand two lines tall before the user has typed anything.
  const narrowViewport =
    typeof window !== 'undefined' && typeof window.matchMedia === 'function'
      ? window.matchMedia('(max-width: 640px)').matches
      : false;

  const placeholder = !canType
    ? (disabledReason ?? 'The composer is unavailable.')
    : running
      ? narrowViewport
        ? queueMode === 'steer'
          ? 'Steer the run…'
          : 'Queue a follow-up…'
        : queueMode === 'steer'
          ? 'The agent is working — type to steer it, or press Enter to queue…'
          : 'The agent is working — type the next thing to do once it finishes…'
      : narrowViewport
        ? 'Ask karucode anything…'
        : 'Ask karucode anything, or describe a change...';

  const attachTitle = !canType
    ? 'Attaching is unavailable until the composer is ready'
    : 'Attach files — or drop them on the composer';

  const approvals = describeApprovals(store.config.value);
  const usageTitle = usageLabel ? `Session usage: ${usageLabel}` : undefined;

  // How full the context window is. Reported, never estimated: the provider's own
  // prompt-token count for the newest call, against the window the selected model
  // declares. Either side missing means no pill, because a percentage drawn from
  // a guess is worse than an empty slot.
  const context = contextUsage(store.chat.value, store.activeContextWindow.value);
  const contextControl = context ? <ContextPill usage={context} onOpenUsage={onOpenUsage} /> : null;
  const characterCounter =
    draft.length > CHAR_COUNTER_THRESHOLD ? (
      <span class={`char-count${draft.length > 12000 ? ' is-over' : ''}`}>
        {draft.length.toLocaleString()} characters
      </span>
    ) : null;

  return (
    <>
      {/* Above the box, inside the dock: a message is only recallable while it is
       * still queued, so the list has to be visible at the moment the user is
       * typing the next one. */}
      <QueuedPanel
        queued={queued ?? []}
        recalling={recalling ?? {}}
        onRecall={(queuedId) => {
          void onRecall?.(queuedId);
        }}
      />
      <form
        class="composer"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
        onDragEnter={(event) => {
          if (!hasFiles(event)) return;
          // Even a disabled composer must cancel a file drag. Otherwise enabling
          // the drop later can still fall through to browser navigation.
          event.preventDefault();
          if (!canType) return;
          dragDepth.current += 1;
          setDragging(true);
        }}
        onDragOver={(event) => {
          if (!hasFiles(event)) return;
          event.preventDefault();
          if (event.dataTransfer) event.dataTransfer.dropEffect = canType ? 'copy' : 'none';
          if (!canType) return;
        }}
        onDragLeave={() => {
          // Some browsers report empty `types` on dragleave, so the counter — not
          // the payload — decides when the overlay goes away.
          if (dragDepth.current === 0) return;
          dragDepth.current -= 1;
          if (dragDepth.current === 0) setDragging(false);
        }}
        onDrop={(event) => {
          if (!hasFiles(event)) return;
          // Cancel every file drop, including when the composer is disabled:
          // ignoring it would make the browser open the dropped local file.
          event.preventDefault();
          const files = event.dataTransfer?.files;
          resetDrag();
          if (!canType) return;
          if (files && files.length > 0) void addFiles(Array.from(files));
        }}
      >
        {menuOpen && mention ? (
          <FileMentionMenu
            id={MENTION_MENU_ID}
            items={ranked}
            activeIndex={activeIndex}
            query={mention.query}
            loading={indexLoading && ranked.length === 0}
            error={ranked.length === 0 ? indexError : null}
            onPick={chooseMention}
            onHover={setActiveOption}
            title={mentionLabel === 'projects' ? 'Projects' : undefined}
          />
        ) : null}

        <div class="compose-field">
          <label class="sr-only" for="composer-input">
            Message karucode
          </label>
          <textarea
            id="composer-input"
            ref={textarea}
            class="composer-input"
            rows={1}
            value={draft}
            placeholder={placeholder}
            disabled={!canType}
            role="combobox"
            aria-expanded={menuOpen}
            aria-controls={menuOpen ? MENTION_MENU_ID : undefined}
            aria-autocomplete="list"
            aria-activedescendant={
              menuOpen && ranked[activeIndex] ? mentionOptionId(MENTION_MENU_ID, activeIndex) : undefined
            }
            aria-describedby="composer-status"
            onInput={(event) => {
              const element = event.currentTarget;
              setDraft(element.value);
              syncCaret(element);
              setDismissed(false);
            }}
            onClick={(event) => syncCaret(event.currentTarget)}
            onKeyUp={(event) => {
              if (event.key.startsWith('Arrow') || event.key === 'Home' || event.key === 'End') {
                syncCaret(event.currentTarget);
              }
            }}
            onPaste={(event) => {
              const files = event.clipboardData?.files;
              if (canType && files && files.length > 0) {
                void addFiles(Array.from(files));
              }
            }}
            onKeyDown={(event) => {
              if (menuOpen && mention) {
                if (event.key === 'ArrowDown' && ranked.length > 0) {
                  event.preventDefault();
                  setActiveOption((current) => (current + 1) % ranked.length);
                  return;
                }
                if (event.key === 'ArrowUp' && ranked.length > 0) {
                  event.preventDefault();
                  setActiveOption((current) => (current - 1 + ranked.length) % ranked.length);
                  return;
                }
                if (
                  (event.key === 'Enter' || event.key === 'Tab') &&
                  !event.metaKey &&
                  !event.ctrlKey &&
                  ranked.length > 0
                ) {
                  event.preventDefault();
                  const chosen = ranked[activeIndex];
                  if (chosen) chooseMention(chosen);
                  return;
                }
                if (event.key === 'Escape') {
                  event.preventDefault();
                  event.stopPropagation();
                  setDismissed(true);
                  return;
                }
              }
              if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
                event.preventDefault();
                void submit();
              } else if (event.key === 'Escape' && running) {
                event.preventDefault();
                event.stopPropagation();
                if (!stopping) onStop();
              }
            }}
          />
        </div>

        {attachments.length > 0 ? (
          <ul class="composer-attachments" aria-label="Attached files">
            {attachments.map((attachment) => (
              <li
                key={attachment.id}
                class={`attachment-chip attachment-chip-${attachment.status}`}
                data-mode={attachment.mode}
              >
                <span class="attachment-chip-icon" aria-hidden="true">
                  <FileIcon size={12} />
                </span>
                <span class="attachment-chip-body">
                  <span class="attachment-chip-name" title={attachment.path}>
                    {attachment.name}
                  </span>
                  <span class="attachment-chip-meta">{attachmentSummary(attachment)}</span>
                </span>
                <button
                  type="button"
                  class="attachment-chip-remove"
                  onClick={() => dropAttachment(attachment.id)}
                  aria-label={`Remove ${attachment.name}`}
                >
                  <CloseIcon size={11} />
                </button>
              </li>
            ))}
          </ul>
        ) : null}

        <div class={`compose-bottom${running ? ' is-running' : ''}`}>
          <div class="compose-configuration">
            <div class="composer-controls compose-bar-left">
              {isUiHidden(store.pluginUi.value, UI_HIDE_COMPOSER_ATTACH) ? null : (
                <button
                  type="button"
                  class="compose-plus"
                  onClick={() => fileInput.current?.click()}
                  disabled={!canType}
                  aria-label="Attach files"
                  title={attachTitle}
                >
                  <PaperclipIcon size={15} />
                </button>
              )}
              <div
                class="composer-session-pins"
                role="group"
                aria-label="Model and reasoning pins for subsequent runs in this session"
                title="Model and reasoning changes apply to subsequent runs, not a run already in flight."
              >
                <ModelControls />
              </div>
              {/* Controls plugins contributed to this row. Nothing is drawn when
               * no plugin asked for a place here, which is the normal case. */}
              <PluginSlot slot={UI_SLOT_COMPOSER_TOOLBAR} />
            </div>
            <div class="composer-spacer" />
            {running ? null : contextControl}
            {running ? null : characterCounter}
          </div>

          {running ? (
            <>
              <div class="compose-run-actions">
                {contextControl}
                {characterCounter}
                {/* Stop ends the run; this group submits into its explicit queue.
                 * Both stay visible with an empty draft. The mode choice appears
                 * only once there is a message for it to describe. */}
                {hasContent ? <QueueModeToggle mode={queueMode} onChange={setQueueMode} /> : null}
                <button
                  type="submit"
                  class="button button-primary button-queue"
                  disabled={!canQueue}
                  aria-label={
                    queueMode === 'steer'
                      ? 'Queue this message and steer the running turn (Command or Control + Enter)'
                      : 'Queue this follow-up for the next turn (Command or Control + Enter)'
                  }
                  title={QUEUE_TITLE[queueMode]}
                >
                  <ArrowUpIcon size={13} />
                  {queueMode === 'steer' ? 'Steer' : 'Queue'}
                </button>
              </div>
              <button
                type="button"
                class="button button-stop"
                onClick={onStop}
                disabled={stopping}
                aria-label="Stop run (Escape)"
                title={stopping ? 'Stopping…' : 'Stop (Escape)'}
              >
                <StopIcon size={13} />
              </button>
            </>
          ) : (
            <button
              type="submit"
              class="button button-primary button-send"
              disabled={!canSend}
              aria-label="Send message (Command or Control + Enter)"
              title="Send (Command or Control + Enter)"
            >
              <ArrowUpIcon size={14} />
            </button>
          )}
        </div>

        <p class="sr-only" aria-live="polite">
          {announcement}
        </p>

        <input
          ref={fileInput}
          type="file"
          class="composer-file-input"
          multiple
          tabIndex={-1}
          aria-hidden="true"
          onChange={(event) => {
            const input = event.currentTarget;
            const files = input.files ? Array.from(input.files) : [];
            input.value = '';
            void addFiles(files);
          }}
        />

        {dragging ? (
          <div class="composer-drop" aria-hidden="true">
            <span class="composer-drop-inner">
              <PlusIcon size={18} />
              Drop files to attach
            </span>
          </div>
        ) : null}
      </form>

      {/* The mock's `.compose-foot` is a sibling of the box, not its last child:
       * status + hints on the left, the approval policy on the right. */}
      <div class="compose-foot">
        <span class="compose-foot-main">
          <span
            id="composer-status"
            class={`composer-status status-${status.kind}`}
            role="status"
            aria-live="polite"
            title={usageTitle}
          >
            {disabledReason ? (
              <>
                <AlertIcon size={13} />
                <span>{disabledReason}</span>
                <button type="button" class="link-button" onClick={onOpenSettings}>
                  Open settings
                </button>
              </>
            ) : (
              <>
                <span class={`status-dot status-${status.kind}`} aria-hidden="true" />
                <span>{status.label}</span>
              </>
            )}
          </span>

          {disabledReason ? null : (
            <span class="composer-hint" aria-hidden="true">
              {running ? (
                <>
                  <strong>Ctrl Enter</strong> to {queueMode === 'steer' ? 'steer' : 'queue a follow-up'}
                  <span class="composer-hint-sep">·</span>
                  <strong>Esc</strong> to stop
                </>
              ) : (
                <>
                  <strong>Ctrl Enter</strong> to send
                  <span class="composer-hint-sep">·</span>
                  <strong>@</strong> reference {mentionLabel ?? 'files'}
                </>
              )}
            </span>
          )}
        </span>

        {approvals ? (
          <button
            type="button"
            class="compose-approvals"
            title={approvals.title}
            aria-label={`Approvals: ${approvals.label}. Open Settings`}
            onClick={onOpenSettings}
          >
            Approvals: {approvals.label}
            <LockIcon size={11} />
          </button>
        ) : null}
      </div>
    </>
  );
}