import type { RefObject } from 'preact';
import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { attachmentsFromQueue, type Attachment } from '../../lib/attachments';
import * as store from '../../state/store';
import { FORM_SCOPE_LIMIT, trimFormScopes } from './helpers';
import type { ComposerFormState, FieldUpdate, ScopedComposerForm } from './types';

export interface ScopedComposerFormHandle {
  sessionId: string | null;
  projectId: string | null;
  form: ScopedComposerForm;
  draft: string;
  attachments: Attachment[];
  caret: number;
  sending: boolean;
  announcement: string;
  setDraft: (update: FieldUpdate<string>) => void;
  setAttachments: (update: FieldUpdate<Attachment[]>) => void;
  setAnnouncement: (update: FieldUpdate<string>) => void;
  setCaret: (update: FieldUpdate<number>) => void;
  setSending: (update: FieldUpdate<boolean>) => void;
}

export function useScopedComposerForm(
  textarea: RefObject<HTMLTextAreaElement | null>,
): ScopedComposerFormHandle {
  // One record per project/session: a different session in the same project must
  // not inherit a draft, just as the same session must not follow a later project.
  // Async upload/send continuations retain this record, not the next visible one.
  //
  // An unsent draft has no session id, so it is keyed by the draft's own nonce:
  // "New session" twice must be two empty boxes, and a fresh draft must not
  // inherit the text of the one that was just sent.
  const sessionId = store.currentSessionId.value;
  const draftId = store.draftSession.value?.id ?? null;
  const projectId = store.currentProjectId.value;
  const scopeKey = JSON.stringify([projectId, sessionId, draftId]);
  const forms = useRef(new Map<string, ScopedComposerForm>());
  // A draft that becomes a real session (its first send, or a binary attachment
  // that had to create it) is the same conversation under a new key, so the
  // half-written message and its chips move with it. `store.draftToSession` names
  // exactly that transition; any other key change - opening another session,
  // switching project - is a different conversation and starts clean.
  const carry = store.draftToSession.value;
  if (carry !== null && carry.sessionId === sessionId) {
    const from = JSON.stringify([projectId, null, carry.draftId]);
    const carried = forms.current.get(from);
    if (carried) {
      forms.current.set(scopeKey, carried);
      forms.current.delete(from);
    }
  }
  let scopedForm = forms.current.get(scopeKey);
  if (scopedForm) {
    // Re-inserting the scope every time it is drawn *is* the LRU: a `Map`
    // iterates in insertion order, so the first key is the scope nothing has
    // touched for the longest time. Without this, an eviction would throw away
    // the session the reader keeps coming back to.
    forms.current.delete(scopeKey);
    forms.current.set(scopeKey, scopedForm);
  } else {
    scopedForm = {
      value: { draft: '', attachments: [], caret: 0, sending: false, announcement: '' },
    };
    forms.current.set(scopeKey, scopedForm);
  }
  // A record holds the attachments' inlined file bodies, so browsing sessions
  // must not accumulate them: keep the recent window, never the visible box.
  trimFormScopes(forms.current, FORM_SCOPE_LIMIT, scopeKey);
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

  // The carry above has been applied, so forget it: a later navigation must not
  // replay it. Clearing it in an effect rather than during render keeps the write
  // out of the render that reads it (that is a render loop waiting to happen).
  useEffect(() => {
    const pending = store.draftToSession.value;
    if (pending !== null && pending.sessionId === sessionId) store.draftToSession.value = null;
  }, [sessionId, carry]);

  // A scope nothing can reach again is a record holding a draft - and whole
  // inlined files - for no one: a session the rail no longer lists because it
  // was deleted, or a draft replaced by the next "New session" (a sent draft has
  // already been carried to its session above, so its old key is gone by now).
  //
  // Only the visible project's rows are consulted: `store.sessions` holds *that*
  // project's sessions, and a parked draft in another project has to survive the
  // switch, which is the reason this map exists. An empty or still-loading list
  // is a refresh in progress, not a mass deletion, so it prunes nothing; and the
  // visible scope is never pruned, because a session created moments ago is not
  // in the rail's rows yet.
  const sessionRows = store.sessions.value;
  useEffect(() => {
    if (store.sessionsLoading.value || sessionRows.length === 0) return;
    const live = new Set(sessionRows.map((row) => row.id));
    // Only an *empty* unreachable record is dropped. A parked draft is unsent
    // typing: losing it because its session was deleted is worse than keeping a
    // record the reader cannot reach, and the count cap above still bounds the
    // map (it sheds the coldest). A record with text, attachments or a send in
    // flight is therefore kept and left to that cap.
    const unreachable: string[] = [];
    for (const key of Array.from(forms.current.keys())) {
      if (key === scopeKey) continue;
      const [project, session, draft] = JSON.parse(key) as [
        string | null,
        string | null,
        number | null,
      ];
      if (project !== projectId) continue;
      const stale =
        session !== null
          ? !live.has(session)
          : draft !== null && draftId !== null && draft !== draftId;
      if (stale) unreachable.push(key);
    }
    const isEmpty = (key: string): boolean => {
      const value = forms.current.get(key)?.value;
      return (
        value === undefined ||
        (value.draft.trim() === '' && value.attachments.length === 0 && !value.sending)
      );
    };
    for (const key of unreachable) {
      if (isEmpty(key)) forms.current.delete(key);
    }
  }, [sessionRows, projectId, scopeKey, draftId]);

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

  return {
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
  };
}
