import { ApiError, apiRequest, apiRequestBlob, withQuery } from '../client';
import type { Project, ProjectListResponse } from '../types/projects';
import type { CallOptions } from './common';

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

export function listProjects(options: CallOptions = {}): Promise<ProjectListResponse> {
  return apiRequest<ProjectListResponse>('/api/projects', options);
}

export function createProject(
  path: string,
  name?: string,
  options: CallOptions = {},
): Promise<Project> {
  return apiRequest<{ project: Project }>('/api/projects', {
    ...options,
    method: 'POST',
    body: name ? { path, name } : { path },
  }).then((r) => r.project);
}

export function renameProject(
  id: string,
  name: string,
  options: CallOptions = {},
): Promise<Project> {
  return apiRequest<{ project: Project }>(`/api/projects/${encodeURIComponent(id)}`, {
    ...options,
    method: 'PATCH',
    body: { name },
  }).then((r) => r.project);
}

export function updateProjectResultPanelWidth(
  id: string,
  width: number,
  options: CallOptions = {},
): Promise<Project> {
  return apiRequest<{ project: Project }>(`/api/projects/${encodeURIComponent(id)}/preferences`, {
    ...options,
    method: 'PATCH',
    body: { result_panel_width: Math.round(width) },
  }).then((r) => r.project);
}

export function deleteProject(
  id: string,
  force = false,
  options: CallOptions = {},
): Promise<{ deleted: boolean; deleted_sessions: number }> {
  return apiRequest<{ deleted: boolean; deleted_sessions: number }>(
    withQuery(`/api/projects/${encodeURIComponent(id)}`, { force: force ? 'true' : undefined }),
    { ...options, method: 'DELETE' },
  );
}

/** How large an icon the rail is willing to draw. */
export const MAX_PROJECT_ICON_BYTES = 512 * 1024;

/**
 * The icon bytes for a project, or `null` when it ships none.
 *
 * The bytes are fetched through the shared client rather than drawn from an
 * `<img src>`: an `<img>` cannot send the `Authorization` header, so it would
 * break on a deployment with `KARUCODE_AUTH_TOKEN` set (only `/api/events`
 * accepts the token in the query string). A `null` result is the normal case —
 * most projects have no favicon — and is not an error.
 */
export async function getProjectIcon(
  id: string,
  options: CallOptions = {},
): Promise<Blob | null> {
  try {
    return await apiRequestBlob(
      withQuery(`/api/projects/${encodeURIComponent(id)}/favicon`, {
        max_bytes: MAX_PROJECT_ICON_BYTES,
      }),
      options,
    );
  } catch (error) {
    // `204 No Content` is the server's quiet "this project ships no icon" — the
    // normal case, which the rail answers with initials. A 404 is accepted too so
    // an older server keeps working.
    if (error instanceof ApiError && error.status === 404) return null;
    throw error;
  }
}
