import { toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import { emptyChatState } from '../chat';
import * as store from '../store';
import * as workspaceTabs from '../workspaceTabs';
import {
  clearSelection,
  enterAuthState,
  isUnauthorized,
  mergeSummary,
  rt,
  summaryFromSession,
} from './shared';

/**
 * Start a new conversation without creating it.
 *
 * This is what every "New session" entry point calls: the rail, the collapsed
 * rail's fly-out, the palette and `Ctrl/Cmd+N`. Nothing is written to the server
 * here - a press that is immediately abandoned must not leave an empty session
 * behind, which is exactly what creating on the click used to do. The session is
 * created by `sendChatMessage` when the first message is sent.
 *
 * It is still a navigation intent: it supersedes a pending create or a project
 * switch's automatic "open latest", so a stale continuation cannot open a session
 * over the draft the reader just started.
 */
export function startDraftSession(options: { projectId?: string | null } = {}): void {
  const projectId = options.projectId === undefined ? store.currentProjectId.value : options.projectId;
  if (
    store.draftSession.value !== null &&
    store.currentSessionId.value === null &&
    store.currentProjectId.value === projectId
  ) {
    // Already drafting in this project: a second press is "focus the box", not a
    // second draft (and must not throw away anything already typed).
    store.mobileSheet.value = null;
    store.composerFocusTick.value += 1;
    return;
  }
  rt.navigationGeneration += 1;
  rt.sessionGeneration += 1;
  clearSelection();
  store.draftSession.value = { id: ++rt.draftSequence, projectId, model: null };
  store.mobileSheet.value = null;
  workspaceTabs.selectWorkspaceView('chat');
  store.composerFocusTick.value += 1;
}

/**
 * Create the session a draft promised and make it the open one.
 *
 * Returns the new id, or `null` when the row could not be created or when the
 * reader navigated away while it was being written. In both cases the caller must
 * not send: a failure keeps the draft (and the composer's text) exactly where it
 * was so the message can be tried again.
 *
 * At most one create is in flight per draft: the first send and a binary
 * attachment uploaded at the same moment must not create two sessions.
 *
 * The generation checks are deliberately the same ones an eager create used: a
 * create that lands after a project switch, a deliberate close or a later
 * selection is not allowed to yank the workspace back to it.
 */
export function createDraftSession(draft: store.DraftSession): Promise<string | null> {
  if (rt.draftCreation?.draftId !== draft.id) {
    const promise = createDraftSessionOnce(draft);
    rt.draftCreation = { draftId: draft.id, promise };
    void promise.finally(() => {
      if (rt.draftCreation?.promise === promise) rt.draftCreation = null;
    });
  }
  return rt.draftCreation.promise;
}

async function createDraftSessionOnce(draft: store.DraftSession): Promise<string | null> {
  const projectRead = rt.projectGeneration;
  const sessionRead = rt.sessionGeneration;
  const navigationOwner = rt.navigationGeneration;
  try {
    // The pin (when the reader chose one) rides in the create request, so the
    // first message already runs on the model the pills showed.
    const session = await api.createSession(
      undefined,
      draft.projectId,
      draft.model ? { model: draft.model } : {},
    );
    if (
      store.draftSession.value?.id !== draft.id ||
      projectRead !== rt.projectGeneration ||
      sessionRead !== rt.sessionGeneration ||
      navigationOwner !== rt.navigationGeneration ||
      draft.projectId !== store.currentProjectId.value
    ) {
      // The row exists on the server, so the next list read shows it, but this
      // browser is no longer looking at that conversation. Leave navigation alone.
      return null;
    }
    // The creation is the newest owner of this workspace's chat reads: anything
    // still in flight for the previous selection must not be committed over it.
    rt.sessionGeneration += 1;
    store.draftSession.value = null;
    store.currentSessionId.value = session.id;
    // The box the reader was typing in moves with the conversation: the composer
    // keys its draft text and chips by (project, session, draft), so it is told
    // which pair to carry across.
    store.draftToSession.value = { draftId: draft.id, sessionId: session.id };
    store.writeLastSessionId(session.id);
    store.chat.value = emptyChatState(session.id);
    store.chatLoading.value = false;
    store.chatError.value = null;
    store.sessionInfo.value = null;
    store.sessionInfoFor.value = null;
    store.sessionInfoError.value = null;
    // The create response carries the pin (when one was sent) and `mergeSummary`
    // is the one place that records a session's answer for the pills.
    mergeSummary(summaryFromSession(session));
    return session.id;
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
      return null;
    }
    store.pushToast(toErrorMessage(error));
    return null;
  }
}

/**
 * Create the draft's session now, for a surface that cannot work without an id.
 *
 * The composer calls this when a file has to be *uploaded* rather than inlined:
 * `POST /api/workspace/upload` stores it under `.karucode/attachments/<session>/`,
 * so the session has to exist before the bytes can be sent. Attaching a real file
 * is already work the reader means to keep, so this is the one way a conversation
 * can be created without a message. It returns the open session's id unchanged
 * when there is no draft (or no session at all, which the caller reports).
 */
export function materializeDraftSession(): Promise<string | null> {
  const draft = store.draftSession.value;
  if (!draft) return Promise.resolve(store.currentSessionId.value);
  return createDraftSession(draft);
}

/** Put text in the composer and focus it (used by the empty state's prompts). */
export function fillComposer(text: string): void {
  // The composer subscribes to `composerDraft` itself, so this lands even while
  // it is already mounted. The tick is a cheap second nudge: any listener that
  // only knows to "focus the composer" still fires, and it is idempotent.
  store.composerDraft.value = text;
  store.composerFocusTick.value += 1;
}
