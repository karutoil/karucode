import type { RefObject, TargetedKeyboardEvent } from 'preact';
import type { QueuedAttachment, QueuedKind } from '../../api/types';
import {
  attachForQueue,
  canSendAttachments,
  composeMessage,
  type Attachment,
} from '../../lib/attachments';
import { isImeComposing } from '../../lib/keyboard';
import { QUEUE_LABEL } from './helpers';
import type { FieldUpdate, ScopedComposerForm } from './types';
import type { ComposerPopoversHandle } from './useComposerPopovers';

export interface ComposerInputHandle {
  canSend: boolean;
  canQueue: boolean;
  canCommand: boolean;
  hasContent: boolean;
  submit: () => Promise<void>;
  syncCaret: (element: HTMLTextAreaElement) => void;
  handleKeyDown: (event: TargetedKeyboardEvent<HTMLTextAreaElement>) => void;
  /**
   * Start a new line at the caret and keep the box focused - the visible
   * counterpart of Shift+Enter for a device that has no Shift key (CO-09).
   */
  insertLineBreak: () => void;
}

export function useComposerInput(options: {
  form: ScopedComposerForm;
  draft: string;
  attachments: Attachment[];
  sending: boolean;
  canType: boolean;
  running: boolean;
  stopping: boolean;
  enterSends: boolean;
  queueMode: QueuedKind;
  textarea: RefObject<HTMLTextAreaElement | null>;
  popovers: ComposerPopoversHandle;
  onSend: (text: string) => Promise<boolean>;
  onCommand?: (text: string) => Promise<boolean>;
  onQueue?: (
    mode: QueuedKind,
    message: { content: string; draft: string; attachments: QueuedAttachment[] },
  ) => Promise<boolean>;
  onStop: () => void;
  setDraft: (update: FieldUpdate<string>) => void;
  setCaret: (update: FieldUpdate<number>) => void;
  setAttachments: (update: FieldUpdate<Attachment[]>) => void;
  setAnnouncement: (update: FieldUpdate<string>) => void;
  setSending: (update: FieldUpdate<boolean>) => void;
}): ComposerInputHandle {
  const {
    form,
    draft,
    attachments,
    sending,
    canType,
    running,
    stopping,
    enterSends,
    queueMode,
    textarea,
    popovers,
    onSend,
    onCommand,
    onQueue,
    onStop,
    setDraft,
    setCaret,
    setAttachments,
    setAnnouncement,
    setSending,
  } = options;

  const attachmentReady = canSendAttachments(attachments);
  const hasContent =
    draft.trim() !== '' || attachments.some((attachment) => attachment.status === 'ready');

  // While a run is in flight the same box sends into the queue instead of
  // starting one, because starting one is exactly what the API refuses with a
  // 409. `canQueue` is therefore the "running" half of `canSend`.
  //
  // `stopping` gates queueing too, and for the same reason: a stop is in flight,
  // so a steer has no run to fold into and a follow-up has no turn to wait for
  // yet - the server answers 409 and the toast is noise. The box stays usable
  // (the draft is kept) and the controls come back when the stop lands.
  const canQueue =
    canType &&
    running &&
    !stopping &&
    !sending &&
    attachmentReady &&
    hasContent &&
    onQueue !== undefined;
  const canSend = canType && !running && !sending && attachmentReady && hasContent;
  // A slash command is not a prompt: the server answers it in place, it never
  // starts a run, and it has to work while one is in flight (`/stop` is the case
  // the whole feature exists for). It also carries no attachments - the text after
  // the command would become its arguments.
  const { slashCommand } = popovers;
  const canCommand = canType && !sending && slashCommand !== null && attachments.length === 0;

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
    popovers.setDismissed(true);
    setAnnouncement(message);
  };

  const submit = async (): Promise<void> => {
    // A command carries no attachments: the text after the command is its
    // arguments, so an attachment would be read as arguments and never reach the
    // model. The command's reply clears the composer, so the reader would simply
    // lose the file. Refuse, and keep both the draft and the attachments.
    if (slashCommand !== null && attachments.length > 0) {
      setAnnouncement(
        `/${slashCommand.name} cannot carry attachments. Remove them to run the command, or send them with a message.`,
      );
      return;
    }
    // A command is answered by the server and is never queued: queueing it behind
    // the run it is trying to change is exactly the wrong behaviour. This branch
    // comes first for that reason.
    if (slashCommand !== null) {
      if (!canType || sending) return;
      const submitted = { draft, attachments };
      setSending(true);
      try {
        const accepted = await (onCommand ?? onSend)(draft.trim());
        if (accepted) clearComposer(`Ran /${slashCommand.name}.`, submitted);
      } finally {
        setSending(false);
      }
      return;
    }
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

  /**
   * Start a new line at the caret, for a chord the browser will not type.
   *
   * Shift+Enter is typed by the browser itself. A Command/Ctrl+Enter produces no
   * text at all in Chrome or Firefox, so when Enter is the send key the composer
   * has to write the break into the draft itself - otherwise the setting's own
   * hint would promise something the key does not do. Going through the draft
   * (rather than editing `value` directly) keeps the controlled textarea, the
   * auto-grow re-measurement and the text that is finally sent the same string.
   */
  const insertLineBreak = (element: HTMLTextAreaElement): void => {
    const start = element.selectionStart ?? element.value.length;
    const end = element.selectionEnd ?? start;
    setDraft(`${element.value.slice(0, start)}\n${element.value.slice(end)}`);
    // Writing the controlled value back drops the caret at the end of the text,
    // so put it after the newline once that render has landed.
    requestAnimationFrame(() => {
      textarea.current?.setSelectionRange(start + 1, start + 1);
    });
  };

  /**
   * The button's version of the same thing: it takes the caret from the live
   * field and hands focus back, because the tap moved focus to the button and a
   * phone would otherwise close the keyboard the writer is still using.
   */
  const insertLineBreakAtCaret = (): void => {
    const element = textarea.current;
    if (!element) return;
    element.focus();
    insertLineBreak(element);
  };

  const handleKeyDown = (event: TargetedKeyboardEvent<HTMLTextAreaElement>): void => {
    // An IME owns these keys while it is composing: Enter commits a
    // candidate, the arrows walk the candidate list and Escape cancels
    // the composition. Every branch below - the `/` menu, the `@` menu,
    // send, queue, stop - would act on half a word. The guard is first
    // because the two popovers used to sit above the send path's own
    // check and ate the commit key before it could reach the IME.
    if (isImeComposing(event)) return;
    // The `/` menu owns the arrows and Enter while it is open. Enter only
    // completes when a row is highlighted: with no match the key must
    // fall through and send the message, which is what a path or a
    // sentence that happens to start with `/` is.
    if (popovers.slashOpen) {
      if (event.key === 'ArrowDown' && popovers.rankedCommands.length > 0) {
        event.preventDefault();
        popovers.setSlashIndex((current) => (current + 1) % popovers.rankedCommands.length);
        return;
      }
      if (event.key === 'ArrowUp' && popovers.rankedCommands.length > 0) {
        event.preventDefault();
        popovers.setSlashIndex(
          (current) =>
            (current - 1 + popovers.rankedCommands.length) % popovers.rankedCommands.length,
        );
        return;
      }
      if (
        (event.key === 'Enter' || event.key === 'Tab') &&
        !event.metaKey &&
        !event.ctrlKey &&
        popovers.rankedCommands.length > 0
      ) {
        event.preventDefault();
        const chosen = popovers.rankedCommands[popovers.slashActiveIndex];
        if (chosen) popovers.chooseCommand(chosen);
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        popovers.setSlashDismissed(true);
        return;
      }
    }
    if (popovers.menuOpen && popovers.mention) {
      if (event.key === 'ArrowDown' && popovers.ranked.length > 0) {
        event.preventDefault();
        popovers.setActiveOption((current) => (current + 1) % popovers.ranked.length);
        return;
      }
      if (event.key === 'ArrowUp' && popovers.ranked.length > 0) {
        event.preventDefault();
        popovers.setActiveOption(
          (current) => (current - 1 + popovers.ranked.length) % popovers.ranked.length,
        );
        return;
      }
      if (
        (event.key === 'Enter' || event.key === 'Tab') &&
        !event.metaKey &&
        !event.ctrlKey &&
        popovers.ranked.length > 0
      ) {
        event.preventDefault();
        const chosen = popovers.ranked[popovers.activeIndex];
        if (chosen) popovers.chooseMention(chosen);
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        popovers.setDismissed(true);
        return;
      }
    }
    if (event.key === 'Enter') {
      // An IME's Enter confirms the composition it is part of, not the
      // message: Japanese, Chinese and Korean input all end a word this
      // way, and sending there would submit half of one. That guard is
      // the first thing this handler does, so both popovers above obey
      // it too.
      const chord = event.metaKey || event.ctrlKey;
      if (enterSends) {
        // Enter sends; Shift and Alt start a line the way the browser
        // already knows how. With nothing to send, Enter falls through
        // to a line break too, rather than swallowing the key.
        if (event.shiftKey || event.altKey) return;
        if (chord) {
          // No browser types a line break for a Command/Ctrl chord
          // (Chrome and Firefox produce no text at all for it), so
          // "the chord starts a new line" is a promise this handler has
          // to keep itself.
          event.preventDefault();
          insertLineBreak(event.currentTarget);
          return;
        }
        if (!(canCommand || (running ? canQueue : canSend))) return;
        event.preventDefault();
        void submit();
        return;
      }
      // Default: a plain Enter is a line break, the chord sends.
      if (!chord) return;
      event.preventDefault();
      void submit();
    } else if (event.key === 'Escape' && running) {
      event.preventDefault();
      event.stopPropagation();
      if (!stopping) onStop();
    }
  };

  return {
    canSend,
    canQueue,
    canCommand,
    hasContent,
    submit,
    syncCaret,
    handleKeyDown,
    insertLineBreak: insertLineBreakAtCaret,
  };
}
