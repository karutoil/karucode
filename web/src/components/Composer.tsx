import type { JSX } from 'preact';
import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import type { QueuedKind } from '../api/types';
import { contextUsage } from '../state/selectors';
import * as store from '../state/store';
import { AttachmentStrip } from '../features/composer/AttachmentStrip';
import { ComposerFooter } from '../features/composer/ComposerFooter';
import { ComposerToolbar } from '../features/composer/ComposerToolbar';
import { ContextPill } from '../features/composer/ContextPill';
import {
  CHAR_COUNTER_THRESHOLD,
  composerPlaceholder,
  FORM_SCOPE_LIMIT,
  MENTION_MENU_ID,
  sendChordLabels,
  SLASH_MENU_ID,
  trimFormScopes,
} from '../features/composer/helpers';
import { QueuedPanel } from '../features/composer/QueueTray';
import type { ComposerProps, UploadContext } from '../features/composer/types';
import { useComposerAttachments } from '../features/composer/useComposerAttachments';
import { useComposerInput } from '../features/composer/useComposerInput';
import { useComposerPopovers } from '../features/composer/useComposerPopovers';
import { useScopedComposerForm } from '../features/composer/useScopedComposerForm';
import { FileMentionMenu, mentionOptionId } from './FileMentionMenu';
import { PlusIcon } from './icons';
import { SlashMenu, slashOptionId } from './SlashMenu';

export { FORM_SCOPE_LIMIT, trimFormScopes };
export type { ComposerProps, UploadContext };

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
  onCommand,
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
  loadCommands,
  mentionLabel,
  uploadFile,
}: ComposerProps): JSX.Element {
  const textarea = useRef<HTMLTextAreaElement>(null);
  const {
    sessionId,
    projectId,
    form,
    draft,
    attachments,
    caret,
    sending,
    announcement,
    setDraft,
    setAttachments,
    setAnnouncement,
    setCaret,
    setSending,
  } = useScopedComposerForm(textarea);

  /**
   * Which queue the next message goes into while a run is in flight.
   *
   * `steer` is the default because it is the more useful of the two mid-run ("also
   * check the fixtures"), and a follow-up is the rarer intent: with nothing
   * running the toggle is hidden and this value is unused, because sending *is*
   * the follow-up then.
   */
  const [queueMode, setQueueMode] = useState<QueuedKind>('steer');

  const popovers = useComposerPopovers({
    draft,
    caret,
    projectId,
    textarea,
    setDraft,
    setCaret,
    loadMentionTree,
    loadCommands,
  });

  const canType = disabledReason === null;

  const {
    dragging,
    fileInput,
    addFiles,
    dropAttachment,
    onDragEnter,
    onDragOver,
    onDragLeave,
    onDrop,
    onPaste,
  } = useComposerAttachments({
    sessionId,
    projectId,
    canType,
    disabledReason,
    attachments,
    setAttachments,
    setAnnouncement,
    uploadFile,
    onUploaded: popovers.appendUploadedSuggestion,
  });

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
    // Narrow layouts are sheets and phones: moving focus there would pop the
    // on-screen keyboard over the conversation the reader just switched to.
    if (store.narrowViewport.value) return;
    const timer = window.setTimeout(() => textarea.current?.focus(), 30);
    return () => window.clearTimeout(timer);
  }, [focusTick]);

  // A stop changes the stop button's label and disables it, which a screen reader
  // does not announce on its own - and the queue controls going quiet looks like a
  // bug unless the wait is audible. Say it once in the live region; the composer
  // itself does not re-render into a spinner.
  useEffect(() => {
    if (stopping) setAnnouncement('Stopping…');
    // `setAnnouncement` writes the scoped form, not component state, so this
    // effect depends only on the stop flag.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stopping]);

  // Which Enter sends. Read from the store during render, so switching it in
  // Settings applies to the very next key press with no reload - and so the hint,
  // the buttons' names and the behaviour can never disagree.
  const enterSends = store.sendKey.value === 'enter';
  const { sendChord, sendChordLong } = sendChordLabels(enterSends);

  const { canSend, canQueue, canCommand, hasContent, submit, syncCaret, handleKeyDown } =
    useComposerInput({
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
    });

  // Phones get a short placeholder: at 16px (the iOS no-zoom size) the desktop
  // sentence wraps to two lines in a 390px column, and an empty dock would then
  // stand two lines tall before the user has typed anything. Read from the store
  // rather than `matchMedia` here: this is decided during render, and a phone
  // held sideways (or a window narrowed after load) has to re-decide.
  const narrowViewport = store.phoneViewport.value;
  const placeholder = composerPlaceholder({
    canType,
    disabledReason,
    running,
    narrowViewport,
    queueMode,
  });

  const attachTitle = !canType
    ? 'Attaching is unavailable until the composer is ready'
    : 'Attach files — or drop them on the composer';

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
        onDragEnter={onDragEnter}
        onDragOver={onDragOver}
        onDragLeave={onDragLeave}
        onDrop={onDrop}
      >
        {popovers.menuOpen && popovers.mention ? (
          <FileMentionMenu
            id={MENTION_MENU_ID}
            items={popovers.ranked}
            activeIndex={popovers.activeIndex}
            query={popovers.mention.query}
            loading={popovers.indexLoading && popovers.ranked.length === 0}
            error={popovers.ranked.length === 0 ? popovers.indexError : null}
            onPick={popovers.chooseMention}
            onHover={popovers.setActiveOption}
            title={mentionLabel === 'projects' ? 'Projects' : undefined}
          />
        ) : null}

        {popovers.slashOpen ? (
          <SlashMenu
            id={SLASH_MENU_ID}
            items={popovers.rankedCommands}
            activeIndex={popovers.slashActiveIndex}
            query={popovers.slashQueryText ?? ''}
            loading={popovers.projectCatalog.length === 0 && popovers.catalogError === null}
            error={popovers.projectCatalog.length === 0 ? popovers.catalogError : null}
            onPick={popovers.chooseCommand}
            onHover={popovers.setSlashIndex}
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
            // A textarea's only role is `textbox` (HTML-ARIA, and the JSX types
            // say the same), so the popup is described with the properties a
            // textbox *does* support: `aria-haspopup` announces that there is a
            // list, `aria-controls`/`aria-activedescendant` name it and the armed
            // row, `aria-autocomplete` says the list narrows as you type. What is
            // deliberately absent is `aria-expanded`, a combobox-only property
            // that assistive tech drops on a plain text field: claiming it made
            // the accessibility tree describe a control this is not.
            aria-haspopup="listbox"
            aria-controls={
              popovers.slashOpen
                ? SLASH_MENU_ID
                : popovers.menuOpen
                  ? MENTION_MENU_ID
                  : undefined
            }
            aria-autocomplete="list"
            aria-activedescendant={
              popovers.slashOpen
                ? popovers.rankedCommands[popovers.slashActiveIndex]
                  ? slashOptionId(SLASH_MENU_ID, popovers.slashActiveIndex)
                  : undefined
                : popovers.menuOpen && popovers.ranked[popovers.activeIndex]
                  ? mentionOptionId(MENTION_MENU_ID, popovers.activeIndex)
                  : undefined
            }
            aria-describedby="composer-status"
            onInput={(event) => {
              const element = event.currentTarget;
              setDraft(element.value);
              syncCaret(element);
              popovers.setDismissed(false);
              popovers.setSlashDismissed(false);
            }}
            onClick={(event) => syncCaret(event.currentTarget)}
            onKeyUp={(event) => {
              if (event.key.startsWith('Arrow') || event.key === 'Home' || event.key === 'End') {
                syncCaret(event.currentTarget);
              }
            }}
            onPaste={onPaste}
            onKeyDown={handleKeyDown}
          />
        </div>

        <AttachmentStrip attachments={attachments} onRemove={dropAttachment} />

        <ComposerToolbar
          running={running}
          stopping={stopping}
          canType={canType}
          canSend={canSend}
          canQueue={canQueue}
          canCommand={canCommand}
          hasContent={hasContent}
          slashCommand={popovers.slashCommand}
          queueMode={queueMode}
          onQueueModeChange={setQueueMode}
          sendChordLong={sendChordLong}
          attachTitle={attachTitle}
          fileInput={fileInput}
          contextControl={contextControl}
          characterCounter={characterCounter}
          onStop={onStop}
        />

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

      <ComposerFooter
        status={status}
        disabledReason={disabledReason}
        running={running}
        queueMode={queueMode}
        sendChord={sendChord}
        mentionLabel={mentionLabel}
        usageLabel={usageLabel}
        onOpenSettings={onOpenSettings}
      />
    </>
  );
}
