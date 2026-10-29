import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import type { Activity } from '../state/selectors';
import * as store from '../state/store';
import * as api from '../api/endpoints';
import { toErrorMessage } from '../api/client';
import type { ApprovalMode, Config } from '../api/types';
import {
  attachmentSummary,
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
import { AlertIcon, ArrowRightIcon, CloseIcon, FileIcon, LockIcon, PlusIcon, StopIcon } from './icons';

/** Short footnote label for the active approval policy. */
const APPROVAL_FOOT_LABEL: Record<ApprovalMode, string> = {
  ask: 'ask',
  ask_commands: 'commands only',
  auto: 'auto',
  read_only: 'read only',
};

const APPROVAL_FOOT_TITLE: Record<ApprovalMode, string> = {
  ask: 'Every mutating tool and shell command asks for approval.',
  ask_commands: 'File writes inside the workspace are allowed; shell commands still ask.',
  auto: 'No approval prompts: every tool call is allowed and recorded.',
  read_only: 'Only read-only tools are allowed; writes and commands are refused.',
};

/**
 * The mock documents its right-hand footnote zone as `Approvals enabled`.
 * Ours says which policy is in force, derived from the live config, and explains
 * the counters in the tooltip.
 */
function describeApprovals(config: Config | null): { label: string; title: string } | null {
  const policy = config?.approvals;
  if (!policy) return null;
  const counts: string[] = [];
  const commands = policy.allow_commands.length;
  const paths = policy.auto_approve_paths.length;
  if (commands > 0) counts.push(`${commands} allowlisted command${commands === 1 ? '' : 's'}`);
  if (paths > 0) counts.push(`${paths} auto-approved path${paths === 1 ? '' : 's'}`);
  return {
    label: APPROVAL_FOOT_LABEL[policy.mode] ?? policy.mode,
    title: [
      APPROVAL_FOOT_TITLE[policy.mode] ?? `Approval mode: ${policy.mode}.`,
      counts.length > 0 ? `${counts.join(', ')}.` : '',
      'Change this in Settings → Approvals.',
    ]
      .filter(Boolean)
      .join(' '),
  };
}

export interface UploadContext {
  session: string | null;
  project: string | null;
}

export interface ComposerProps {
  disabledReason: string | null;
  running: boolean;
  stopping: boolean;
  status: Activity;
  /** Receives the final message text (draft + inlined/referenced attachments). */
  onSend: (text: string) => Promise<boolean>;
  onStop: () => void;
  onOpenSettings: () => void;
  focusTick: number;
  usageLabel?: string;
  /** Override the workspace tree fetch behind `@` suggestions (tests). */
  loadMentionTree?: TreeLoader;
  /** Override the attachment upload call (tests). */
  uploadFile?: (file: File, context: UploadContext) => Promise<{ path: string }>;
}

const CHAR_COUNTER_THRESHOLD = 4000;
/** The mock's `.compose-area{max-height:150px}`; the inline height below wins
 * over CSS, so this constant — not the stylesheet — is what caps the box. */
const MAX_TEXTAREA_HEIGHT = 150;
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
  onStop,
  onOpenSettings,
  focusTick,
  usageLabel,
  loadMentionTree,
  uploadFile,
}: ComposerProps): JSX.Element {
  const [draft, setDraft] = useState('');

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
  const [caret, setCaret] = useState(0);
  const [sending, setSending] = useState(false);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [announcement, setAnnouncement] = useState('');
  const [dragging, setDragging] = useState(false);
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
  const loadingIndex = useRef(false);
  const loadTree = loadMentionTree ?? defaultLoadTree;

  const mention = activeMention(draft, caret);
  const mentionOpen = mention !== null && !dismissed;
  const ranked = mentionOpen && mention ? rankMentions(suggestions, mention.query, MENTION_LIMIT) : [];
  const activeIndex = ranked.length === 0 ? 0 : Math.min(activeOption, ranked.length - 1);
  const menuOpen = mentionOpen && (ranked.length > 0 || indexLoading || indexError !== null);

  useEffect(() => {
    if (!mentionOpen) return;
    if (loadingIndex.current) return;
    const fresh = indexedAt.current > 0 && Date.now() - indexedAt.current < INDEX_TTL_MS;
    if (fresh) return;
    let cancelled = false;
    loadingIndex.current = true;
    setIndexLoading(true);
    setIndexError(null);
    void (async () => {
      try {
        const tree = await loadTree(MENTION_INDEX_DEPTH);
        if (cancelled) return;
        setSuggestions(indexFromTree(tree).filter((entry) => entry.kind === 'file'));
        indexedAt.current = Date.now();
      } catch (error) {
        if (cancelled) return;
        setIndexError(`Could not index the workspace: ${toErrorMessage(error)}`);
      } finally {
        setIndexLoading(false);
        loadingIndex.current = false;
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [mentionOpen, loadTree, suggestions.length]);

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
      const result = await upload(file, {
        session: store.currentSessionId.value,
        project: store.currentProjectId.value,
      });
      updateAttachment(attachment.id, { status: 'ready', path: result.path });
      // The uploaded file is now mentionable without waiting for a refetch.
      setSuggestions((current) =>
        current.some((entry) => entry.path === result.path)
          ? current
          : [...current, { path: result.path, kind: 'file', size: attachment.size }],
      );
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
  useEffect(() => {
    const element = textarea.current;
    if (!element) return;
    element.style.height = 'auto';
    element.style.height = `${Math.min(element.scrollHeight, MAX_TEXTAREA_HEIGHT)}px`;
  }, [draft]);

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
  const canSend =
    canType &&
    !running &&
    !sending &&
    attachmentReady &&
    (draft.trim() !== '' || attachments.some((attachment) => attachment.status === 'ready'));

  const submit = async (): Promise<void> => {
    if (!canSend) return;
    setSending(true);
    try {
      const accepted = await onSend(composeMessage(draft, attachments));
      if (accepted) {
        const sent = attachments.length;
        setDraft('');
        setCaret(0);
        setAttachments([]);
        setDismissed(true);
        setAnnouncement(sent === 0 ? '' : `Sent with ${sent} ${sent === 1 ? 'attachment' : 'attachments'}.`);
      }
    } finally {
      setSending(false);
    }
  };

  const syncCaret = (element: HTMLTextAreaElement): void => {
    setCaret(element.selectionStart ?? element.value.length);
  };

  const placeholder = !canType
    ? (disabledReason ?? 'The composer is unavailable.')
    : running
      ? 'The agent is working — draft the next message while you wait…'
      : 'Ask karucode anything, or describe a change...';

  const attachTitle = !canType
    ? 'Attaching is unavailable until the composer is ready'
    : 'Attach files — or drop them on the composer';

  const approvals = describeApprovals(store.config.value);
  const usageTitle = usageLabel ? `Session usage: ${usageLabel}` : undefined;

  return (
    <>
      <form
        class="composer"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
        onDragEnter={(event) => {
          if (!canType || !hasFiles(event)) return;
          event.preventDefault();
          dragDepth.current += 1;
          setDragging(true);
        }}
        onDragOver={(event) => {
          if (!canType || !hasFiles(event)) return;
          event.preventDefault();
          if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy';
        }}
        onDragLeave={() => {
          // Some browsers report empty `types` on dragleave, so the counter — not
          // the payload — decides when the overlay goes away.
          if (dragDepth.current === 0) return;
          dragDepth.current -= 1;
          if (dragDepth.current === 0) setDragging(false);
        }}
        onDrop={(event) => {
          if (!canType) return;
          // Always cancel the default: an unmatched drop would navigate away.
          event.preventDefault();
          const files = event.dataTransfer?.files;
          resetDrag();
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
          />
        ) : null}

        <div class="compose-top">
          <button
            type="button"
            class="compose-plus"
            onClick={() => fileInput.current?.click()}
            disabled={!canType}
            aria-label="Attach files"
            title={attachTitle}
          >
            <PlusIcon size={18} />
          </button>

          <div class="compose-field">
            <label class="sr-only" for="composer-input">
              Message karucode
            </label>
            <textarea
              id="composer-input"
              ref={textarea}
              class="composer-input"
              rows={2}
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

        <div class="compose-bottom">
          <ModelControls />
          <div class="composer-spacer" />

          {draft.length > CHAR_COUNTER_THRESHOLD ? (
            <span class={`char-count${draft.length > 12000 ? ' is-over' : ''}`}>
              {draft.length.toLocaleString()} characters
            </span>
          ) : null}

          {running ? (
            <button
              type="button"
              class="button button-stop"
              onClick={onStop}
              disabled={stopping}
              aria-label="Stop run (Escape)"
            >
              <StopIcon size={13} />
              {stopping ? 'Stopping…' : 'Stop'}
            </button>
          ) : (
            <button
              type="submit"
              class="button button-primary button-send"
              disabled={!canSend}
              aria-label="Send message (Command or Control + Enter)"
            >
              <ArrowRightIcon size={15} />
              Send
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
                  <strong>Esc</strong> to stop
                </>
              ) : (
                <>
                  <strong>Ctrl Enter</strong> to send
                  <span class="composer-hint-sep">·</span>
                  <strong>@</strong> mention files
                </>
              )}
            </span>
          )}
        </span>

        {approvals ? (
          <span class="compose-approvals" title={approvals.title}>
            Approvals: {approvals.label}
            <LockIcon size={11} />
          </span>
        ) : null}
      </div>
    </>
  );
}