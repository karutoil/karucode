import { ApiError, toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import type {
  ApprovalDecision,
  QueuedAttachment,
  QueuedKind,
  RewindMode,
  RewindPreview,
} from '../../api/types';
import { normalizeQueuedMessage, type MessageItem } from '../chat';
import { markUserRun } from '../eventReducer';
import * as store from '../store';
import { createDraftSession } from './composer';
import { openSession, scheduleSessionsRefresh } from './sessions';
import { enterAuthState, isUnauthorized, nextHandoffToken, replaceOverlay, rt } from './shared';

export async function sendChatMessage(content: string): Promise<boolean> {
  const trimmed = content.trim();
  if (trimmed === '') return false;
  const draft = store.draftSession.value;
  const openId = store.currentSessionId.value;
  // Nothing to send into: no open session and no draft to create one from.
  if (!openId && !draft) return false;
  // The draft has no id yet, so it gets the guard key instead: a second submit
  // while the create is in flight must not create a second session.
  const guard = openId ?? 'draft';
  if (store.chat.value.running || rt.sendInFlight.has(guard)) return false;

  rt.sendInFlight.add(guard);
  let sessionId: string | null = openId;
  try {
    if (sessionId === null) {
      // The first message is what creates the session: see `startDraftSession`.
      // A failure returns `false` and leaves the draft (and the typed text)
      // exactly as it was, so sending again is the whole retry.
      const created = draft ? await createDraftSession(draft) : null;
      if (created === null) return false;
      sessionId = created;
      rt.sendInFlight.add(sessionId);
    }

    const generation = rt.sessionGeneration;
    const projectRead = rt.projectGeneration;
    rt.localMessageSequence += 1;
    const localId = `local:${Date.now()}:${rt.localMessageSequence}`;
    const optimistic: MessageItem = {
      kind: 'message',
      id: localId,
      at: Date.now(),
      role: 'user',
      content: trimmed,
      reasoning: '',
      streaming: false,
      runId: null,
      agentId: null,
    };
    store.chat.value = {
      ...store.chat.value,
      items: [...store.chat.value.items, optimistic],
      running: true,
      runStatus: 'running',
      runError: null,
      iteration: 0,
    };
    // The reader wants to see the answer, so the transcript re-pins to the bottom.
    store.sentMessageTick.value += 1;

    try {
      const result = await api.sendMessage(sessionId, trimmed);
      if (store.currentSessionId.value === sessionId) {
        if (result.mode === 'command') {
          // The server answered this message as a slash command: it stored the
          // command's reply and *no* user row, and started no run. The answer
          // arrives as a `command_result` frame, so the optimistic bubble is a
          // message that does not exist and the run state it set is not a run -
          // leaving either behind stuck the session "running" (every later send
          // returned false at the guard) and left a bubble with nothing behind it.
          const current = store.chat.value;
          store.chat.value = {
            ...current,
            items: current.items.filter((item) => item.id !== localId),
            running: false,
            runStatus: null,
            iteration: 0,
          };
        } else {
          const current = store.chat.value;
          store.chat.value = {
            ...current,
            runId: result.run_id,
            items: markUserRun(current.items, result.run_id),
          };
        }
      }
      // The new row (a draft's first send included) belongs in the rail now: the
      // first message is also what titles an untitled session.
      scheduleSessionsRefresh();
      return true;
    } catch (error) {
      // A silent same-session snapshot is a newer owner too. Check everything before
      // removing the optimistic row, changing run state, reporting an old error, or
      // launching another read: doing that afterwards already clipped the new run.
      if (
        generation !== rt.sessionGeneration ||
        projectRead !== rt.projectGeneration ||
        store.currentSessionId.value !== sessionId
      ) {
        return false;
      }
      const current = store.chat.value;
      store.chat.value = {
        ...current,
        items: current.items.filter((item) => item.id !== localId),
        running: false,
        runStatus: null,
      };
      if (isUnauthorized(error)) {
        enterAuthState();
      } else if (error instanceof ApiError && error.status === 409) {
        store.pushToast('A run is already active in this session.', 'info');
      } else {
        store.pushToast(toErrorMessage(error));
      }
      void openSession(sessionId, { silent: true });
      return false;
    }
  } finally {
    rt.sendInFlight.delete(guard);
    // For a draft the guard is not the session id, so the id it just became is
    // released separately (for a normal send the two are the same key).
    if (sessionId) rt.sendInFlight.delete(sessionId);
  }
}

/**
 * Send one slash command and let the server answer it.
 *
 * Kept apart from {@link sendChatMessage} because a command is not a prompt: it
 * does not start a run, it is allowed **while a run is working** (`/stop` exists
 * for exactly that moment), and its answer arrives as a `command_result` frame
 * rather than as streamed text. Nothing is added optimistically - the frame is
 * what draws the card, and the row it names is already persisted by then, so a
 * reload and a live tab converge.
 *
 * A stale catalog is harmless: if the server does not recognise the command it
 * treats the message as an ordinary prompt, starts a run, and the `run_started`
 * frame draws the turn exactly as a normal send would.
 */
export async function runSlashCommand(content: string): Promise<boolean> {
  const trimmed = content.trim();
  if (trimmed === '') return false;
  const draft = store.draftSession.value;
  const openId = store.currentSessionId.value;
  if (!openId && !draft) return false;
  const guard = openId ?? 'draft';
  if (rt.sendInFlight.has(guard)) return false;

  rt.sendInFlight.add(guard);
  let sessionId: string | null = openId;
  try {
    if (sessionId === null) {
      const created = draft ? await createDraftSession(draft) : null;
      if (created === null) return false;
      sessionId = created;
      rt.sendInFlight.add(sessionId);
    }
    await api.sendMessage(sessionId, trimmed);
    // A command can change the session row itself (a rename, a model pin), so the
    // rail is refreshed on the same schedule a message send uses.
    scheduleSessionsRefresh();
    return true;
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
    return false;
  } finally {
    rt.sendInFlight.delete(guard);
    if (sessionId) rt.sendInFlight.delete(sessionId);
  }
}

export async function stopRun(): Promise<void> {
  const sessionId = store.currentSessionId.value;
  if (!sessionId) return;
  store.stopping.value = true;
  try {
    await api.cancelRun(sessionId);
    scheduleSessionsRefresh();
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
  } finally {
    store.stopping.value = false;
  }
}

/**
 * Send a message into the run that is already working.
 *
 * A *steer* folds into the turn in flight and a *follow-up* starts the run after
 * it; either way the message is queued rather than submitted, so this returns
 * once the server has accepted it and nothing is optimistically added to the
 * transcript. The message appears there only when the run applies it (a steer
 * gets a `steer_applied` frame, a follow-up its own run), which is what keeps the
 * transcript in the order the model actually saw things.
 */
export async function queueChatMessage(
  mode: QueuedKind,
  message: { content: string; draft?: string; attachments?: QueuedAttachment[] },
): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  const generation = rt.sessionGeneration;
  const projectRead = rt.projectGeneration;
  const stillCurrent = () =>
    generation === rt.sessionGeneration &&
    projectRead === rt.projectGeneration &&
    store.currentSessionId.value === sessionId;
  const sameSession = () =>
    projectRead === rt.projectGeneration && store.currentSessionId.value === sessionId;
  if (!sessionId || message.content.trim() === '') return false;
  if (!store.chat.value.running) return false;
  try {
    const result = await api.sendQueuedMessage(sessionId, mode, message);
    // The acceptance carries the entry, so the pending panel is correct even
    // before the `queued_messages` frame arrives (and stays correct if it never
    // does, because a reconnect refetches the snapshot).
    if (sameSession() && result.queued_message) {
      const current = store.chat.value;
      // Normalised on the way in: the acceptance body is the raw struct too, so
      // `draft` may be absent (`normalizeQueuedMessage`).
      const entry = normalizeQueuedMessage(result.queued_message);
      store.chat.value = {
        ...current,
        queued:
          current.queued.some((queued) => queued.id === entry.id) ||
          current.retiredQueuedIds.includes(entry.id)
            ? current.queued
            : [...current.queued, entry],
      };
    }
    return true;
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
    } else if (error instanceof ApiError && error.status === 409) {
      // The run ended between the click and the request. Say so plainly: the
      // message is still in the composer, and sending it again is a plain send.
      store.pushToast(
        'That run finished before the message could be queued. Send it as a normal message instead.',
        'info',
      );
      // Resynchronise: the queue this browser shows is now wrong, and so is the
      // `running` flag it decided from.
      if (stillCurrent()) {
        void openSession(sessionId, { silent: true, force: true, preserveLive: true });
      }
    } else {
      store.pushToast(toErrorMessage(error));
    }
    return false;
  }
}

/**
 * Take a queued message back out of the queue and put it in the composer.
 *
 * The message never reached the model, so this is the undo for "sent too soon":
 * the draft goes back in the textarea and the attachment chips come back, which
 * is why the server hands the entry back rather than just a `deleted: true`.
 */
export async function recallQueuedMessage(queuedId: string): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  const generation = rt.sessionGeneration;
  const projectRead = rt.projectGeneration;
  const stillCurrent = () =>
    generation === rt.sessionGeneration &&
    projectRead === rt.projectGeneration &&
    store.currentSessionId.value === sessionId;
  const sameSession = () =>
    projectRead === rt.projectGeneration && store.currentSessionId.value === sessionId;
  if (!sessionId || queuedId === '') return false;
  if (rt.recallingQueued.has(queuedId)) return false;
  rt.recallingQueued.add(queuedId);
  store.recallingQueued.value = { ...store.recallingQueued.value, [queuedId]: true };
  try {
    const result = await api.recallQueued(sessionId, queuedId);
    const entry = result.recalled;
    if (sameSession()) {
      const current = store.chat.value;
      store.chat.value = {
        ...current,
        queued: current.queued.filter((queued) => queued.id !== queuedId),
        retiredQueuedIds: [...new Set([...current.retiredQueuedIds, queuedId])].slice(-100),
      };
      // The composer consumes the hand-off and appends to whatever the user has
      // typed since, so a recall never silently replaces a fresh draft. The
      // recalled entry is normalised on the way in (`normalizeQueuedMessage`), so
      // `draft` is a string here even when the server omitted it.
      const recalled = normalizeQueuedMessage(entry);
      store.composerHandoff.value = {
        text: recalled.draft ?? recalled.content,
        attachments: recalled.attachments ?? [],
        token: nextHandoffToken(),
      };
      store.composerFocusTick.value += 1;
    }
    return true;
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
    } else if (error instanceof ApiError && error.status === 404) {
      // It landed while the click was in flight. The queue on screen is stale, so
      // refresh instead of resurrecting a message the model already saw.
      store.pushToast('That message was already applied to the run.', 'info');
      if (stillCurrent()) {
        void openSession(sessionId, { silent: true, force: true, preserveLive: true });
      }
    } else {
      store.pushToast(toErrorMessage(error));
    }
    return false;
  } finally {
    rt.recallingQueued.delete(queuedId);
    const next = { ...store.recallingQueued.value };
    delete next[queuedId];
    store.recallingQueued.value = next;
  }
}

export async function resolveApproval(
  approvalId: string,
  decision: Exclude<ApprovalDecision, 'expired'>,
): Promise<boolean> {
  if (store.resolvingApprovals.value[approvalId]) return false;
  store.resolvingApprovals.value = { ...store.resolvingApprovals.value, [approvalId]: true };
  try {
    await api.resolveApproval(approvalId, decision);
    const current = store.chat.value;
    store.chat.value = {
      ...current,
      items: current.items.map((item) =>
        item.kind === 'approval' && item.approval.id === approvalId
          ? {
              ...item,
              approval: { ...item.approval, status: 'resolved', decision },
            }
          : item,
      ),
    };
    return true;
  } catch (error) {
    if (isUnauthorized(error)) {
      enterAuthState();
    } else {
      store.pushToast(toErrorMessage(error));
      if (error instanceof ApiError && error.status === 404) {
        const current = store.chat.value;
        store.chat.value = {
          ...current,
          items: current.items.map((item) =>
            item.kind === 'approval' && item.approval.id === approvalId
              ? { ...item, approval: { ...item.approval, status: 'expired' } }
              : item,
          ),
        };
      }
      // Any other failure leaves the card saying "waiting for this decision",
      // which is a claim about the *server's* state that this call just proved it
      // cannot read. Resolve it the way `sendChatMessage` resolves its own failed
      // send: refetch, silently and forced, so the server's pending-approvals
      // snapshot decides what the card says. Nothing is guessed locally - the
      // approval may still be pending (a network blip), already applied, or
      // expired by the runtime, and only the server knows which.
      const sessionId = store.currentSessionId.value;
      if (sessionId) {
        void openSession(sessionId, { silent: true, force: true, preserveLive: true });
      }
    }
    return false;
  } finally {
    const next = { ...store.resolvingApprovals.value };
    delete next[approvalId];
    store.resolvingApprovals.value = next;
  }
}

export function dismissRunError(): void {
  store.chat.value = { ...store.chat.value, runError: null };
}

/** Open the rewind dialog for one transcript message. */
export function openRewind(target: store.RewindTarget): void {
  if (store.chat.value.running) {
    // A live run holds the conversation in memory; the server refuses the rewind
    // too, so say why here instead of letting the dialog offer something that
    // will come back a 409.
    store.pushToast('Stop the run before rewinding: the agent is still writing.', 'info');
    return;
  }
  if (store.currentSessionId.value === null) return;
  replaceOverlay('rewind');
  rt.rewindGeneration += 1;
  store.rewindTarget.value = target;
}

export function closeRewind(): void {
  if (store.rewindTarget.value !== null) rt.rewindGeneration += 1;
  store.rewindTarget.value = null;
}

/**
 * Whether an answer that just arrived still belongs to the dialog on screen.
 *
 * Deliberately keyed on the target, the session and the project - not on
 * `sessionGeneration`, which every *read* of the session bumps. That counter
 * moves when the event stream reconnects and the open session is re-read
 * silently, and a preview request that happened to be in flight then was thrown
 * away: the dialog reported "the rewind preview could not be loaded" for a
 * message whose preview the server had already answered, while a second look
 * loaded it. A re-read of the same session does not change what a rewind of that
 * message would do; a *different* session, project or target does, and those are
 * still checked.
 */
function rewindStillCurrent(context: {
  generation: number;
  projectRead: number;
  sessionId: string;
  target: store.RewindTarget;
}): boolean {
  return (
    context.generation === rt.rewindGeneration &&
    context.projectRead === rt.projectGeneration &&
    store.currentSessionId.value === context.sessionId &&
    store.rewindTarget.value === context.target
  );
}

/** What a rewind of the open target would do: the file list and the counts. */
export async function loadRewindPreview(): Promise<RewindPreview | null> {
  const sessionId = store.currentSessionId.value;
  const target = store.rewindTarget.value;
  if (!sessionId || !target) return null;
  const context = {
    generation: rt.rewindGeneration,
    projectRead: rt.projectGeneration,
    sessionId,
    target,
  };
  const current = () => rewindStillCurrent(context);
  try {
    const response = await api.previewRewind(sessionId, target.messageId);
    return current() ? response.preview : null;
  } catch (error) {
    if (!current()) return null;
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(toErrorMessage(error));
    return null;
  }
}

/**
 * Perform the rewind the dialog described.
 *
 * Returns true when the server applied it. A `retry` or `note` also starts a
 * run; the transcript is refetched either way, because hiding rows is not
 * something the client can reproduce (the server normalises the rewind point and
 * the journal's effects are not in the transcript).
 */
export async function applyRewind(options: {
  mode: RewindMode;
  revert: boolean;
  note?: string;
}): Promise<boolean> {
  const sessionId = store.currentSessionId.value;
  const target = store.rewindTarget.value;
  if (!sessionId || !target) return false;
  const context = {
    generation: rt.rewindGeneration,
    projectRead: rt.projectGeneration,
    sessionId,
    target,
  };
  const current = () => rewindStillCurrent(context);
  const note = options.note?.trim() ?? '';
  if (options.mode === 'note' && note === '') {
    store.pushToast('Write the note you want the agent to act on.', 'info');
    return false;
  }

  try {
    const response = await api.rewindSession(sessionId, {
      message_id: target.messageId,
      revert: options.revert,
      mode: options.mode,
      ...(note !== '' ? { note } : {}),
    });
    if (!current()) return true;
    rt.rewindGeneration += 1;
    store.rewindTarget.value = null;
    // The reload is what makes the hidden tail disappear; the server owns both
    // the cut and the normalisation. It runs only while this completion still
    // owns its target; an old apply must not navigate away from a later session.
    await openSession(sessionId, { force: true, silent: true });
    scheduleSessionsRefresh();
    // The files the revert could not put back are the interesting part of a
    // successful call: saying nothing would imply a clean revert.
    if (response.files_skipped > 0) {
      store.pushToast(
        `${response.files_restored} file(s) restored, ${response.files_skipped} could not be.`,
        'info',
      );
    } else if (response.files_restored > 0) {
      store.pushToast(`${response.files_restored} file(s) restored.`, 'info');
    } else if (options.mode !== 'none') {
      store.pushToast(
        options.mode === 'retry' ? 'Retrying that request.' : 'Sending your note.',
        'info',
      );
    }
    return true;
  } catch (error) {
    if (!current()) return false;
    if (isUnauthorized(error)) {
      enterAuthState();
    } else if (error instanceof ApiError && error.status === 409) {
      store.pushToast('A run is still active in this session; stop it first.', 'info');
    } else {
      store.pushToast(toErrorMessage(error));
    }
    return false;
  }
}
