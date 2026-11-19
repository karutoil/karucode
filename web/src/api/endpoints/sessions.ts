import { apiRequest, withQuery } from '../client';
import type {
  Session,
  SessionAnnotation,
  SessionDetail,
  SessionInfo,
  SessionModelSelection,
  SessionSummary,
} from '../types/sessions';
import type { CallOptions } from './common';

/** `POST /api/sessions` body extras: the pin a draft chose before it existed. */
export interface CreateSessionOptions extends CallOptions {
  /**
   * Per-session model pin, sent with the creation.
   *
   * `undefined` (the default) omits the field, which is what an unpinned session
   * wants; `null` would ask the server to clear a pin a new session cannot have.
   */
  model?: SessionModelSelection | null;
}

export function listSessions(
  projectId?: string | null,
  options: CallOptions = {},
): Promise<SessionSummary[]> {
  return apiRequest<{ sessions: SessionSummary[] }>(
    withQuery('/api/sessions', { project: projectId ?? undefined }),
    options,
  ).then((r) => r.sessions);
}

/**
 * The rail's list **and** the plugin annotations that shape it.
 *
 * Separate from {@link listSessions} so the many callers that only need the
 * sessions keep their existing shape: the annotations are a display concern and a
 * plugin's answer must never be able to change what those callers see.
 */
export function listSessionsWithAnnotations(
  projectId?: string | null,
  options: CallOptions = {},
): Promise<{ sessions: SessionSummary[]; annotations: SessionAnnotation[] }> {
  return apiRequest<{ sessions: SessionSummary[]; annotations?: SessionAnnotation[] }>(
    withQuery('/api/sessions', { project: projectId ?? undefined }),
    options,
  ).then((r) => ({ sessions: r.sessions, annotations: r.annotations ?? [] }));
}

export function createSession(
  title?: string,
  projectId?: string | null,
  options: CreateSessionOptions = {},
): Promise<Session> {
  const { model, ...call } = options;
  const body: Record<string, unknown> = {};
  if (title) body.title = title;
  if (projectId) body.project = projectId;
  // `POST /api/sessions` validates the pin before it creates anything, so a draft
  // that picked a model is created already pinned: the first message runs on the
  // model the composer showed, with no second request in between.
  if (model !== undefined) body.model = model;
  return apiRequest<Session>('/api/sessions', {
    ...call,
    method: 'POST',
    body,
  });
}

export function getSession(id: string, options: CallOptions = {}, before?: number): Promise<SessionDetail> {
  return apiRequest<SessionDetail>(withQuery(`/api/sessions/${encodeURIComponent(id)}`, { before }), options);
}

/**
 * A session's counts and token totals, without its transcript.
 *
 * Cheap enough for the inspector's Session tab to poll while a run is in
 * flight: it carries no message bodies, unlike `getSession`.
 */
export function getSessionInfo(id: string, options: CallOptions = {}): Promise<SessionInfo> {
  return apiRequest<SessionInfo>(`/api/sessions/${encodeURIComponent(id)}/info`, options);
}

export function renameSession(id: string, title: string, options: CallOptions = {}): Promise<Session> {
  return apiRequest<Session>(`/api/sessions/${encodeURIComponent(id)}`, {
    ...options,
    method: 'PATCH',
    body: { title },
  });
}

/**
 * Pin this session to a provider + model (optionally with a reasoning effort),
 * or pass null to follow the install-wide default again.
 */
export function setSessionModel(
  id: string,
  selection: SessionModelSelection | null,
  options: CallOptions = {},
): Promise<Session> {
  return apiRequest<Session>(`/api/sessions/${encodeURIComponent(id)}/model`, {
    ...options,
    method: 'PATCH',
    body: selection ?? {},
  });
}

/**
 * Set this session's subagent-worktree override, or pass null to follow
 * `config.subagents.worktrees` again.
 *
 * `false` is a real override, not "unset": a session can keep its children in the
 * shared tree after the install turned isolation on.
 */
export function setSessionWorktrees(
  id: string,
  worktrees: boolean | null,
  options: CallOptions = {},
): Promise<Session> {
  return apiRequest<Session>(`/api/sessions/${encodeURIComponent(id)}`, {
    ...options,
    method: 'PATCH',
    body: { worktrees },
  });
}

export function deleteSession(id: string, options: CallOptions = {}): Promise<{ deleted: boolean }> {
  return apiRequest<{ deleted: boolean }>(`/api/sessions/${encodeURIComponent(id)}`, {
    ...options,
    method: 'DELETE',
  });
}
