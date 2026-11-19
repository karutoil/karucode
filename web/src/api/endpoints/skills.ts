import { apiRequest, withQuery } from '../client';
import type { Config } from '../types/config';
import type {
  CommandSpec,
  CommandsResponse,
  SkillCreateRequest,
  SkillDetail,
  SkillListResponse,
  SkillRoot,
  SkillSource,
  SkillSummary,
  SkillUpdateRequest,
  SkillsConfigPatch,
} from '../types/skills';
import type { CallOptions } from './common';

// ---------------------------------------------------------------------------
// Skills (`GET /api/skills` and friends)
// ---------------------------------------------------------------------------

/**
 * The wire spelling of a skill's `source`, mirrored verbatim.
 *
 * `kc_core::skill::SkillSource` serialises as `workspace | user | config |
 * custom`. A payload carrying none of those (an older build, or a proxy
 * answering something else) falls back to the file-backed default rather than
 * inventing a fifth value the rest of the app would not know how to label.
 * `configured` is accepted as an alias because an early copy of the contract used
 * that word for `config`; the server never sends it.
 */
function skillSource(raw: unknown): SkillSource {
  if (raw === 'configured') return 'config';
  return raw === 'user' || raw === 'custom' || raw === 'config' ? raw : 'workspace';
}

/**
 * Read one skill row without trusting the payload.
 *
 * The route is new: a build whose server predates it (or a proxy answering
 * something else entirely) must degrade to an empty panel with a message, not to
 * a render that throws on a missing field. `custom` is derived from `source`
 * when the server omits it, because `source === 'custom'` is the definition the
 * contract gives it.
 */
function readSkill(raw: unknown): SkillSummary | null {
  if (typeof raw !== 'object' || raw === null) return null;
  const row = raw as Record<string, unknown>;
  const id = typeof row.id === 'string' ? row.id : '';
  if (id === '') return null;
  const source = skillSource(row.source);
  const name = typeof row.name === 'string' && row.name !== '' ? row.name : id;
  return {
    id,
    name,
    description: typeof row.description === 'string' ? row.description : '',
    source,
    path: typeof row.path === 'string' ? row.path : null,
    enabled: row.enabled !== false,
    custom: typeof row.custom === 'boolean' ? row.custom : source === 'custom',
  };
}

function readSkillRoot(raw: unknown): SkillRoot | null {
  if (typeof raw !== 'object' || raw === null) return null;
  const row = raw as Record<string, unknown>;
  const path = typeof row.path === 'string' ? row.path : '';
  if (path === '') return null;
  return {
    path,
    source: skillSource(row.source),
    exists: row.exists === true,
    configured: row.configured === true,
    count: typeof row.count === 'number' && Number.isFinite(row.count) ? row.count : 0,
  };
}

function readSkillList(raw: unknown): SkillListResponse {
  const body = (typeof raw === 'object' && raw !== null ? raw : {}) as Record<string, unknown>;
  const skills = Array.isArray(body.skills) ? body.skills : [];
  const roots = Array.isArray(body.roots) ? body.roots : [];
  return {
    enabled: body.enabled !== false,
    auto_detect: body.auto_detect !== false,
    directories: Array.isArray(body.directories)
      ? body.directories.filter((entry): entry is string => typeof entry === 'string')
      : [],
    prompt: body.prompt !== false,
    prompt_max_chars:
      typeof body.prompt_max_chars === 'number' && Number.isFinite(body.prompt_max_chars)
        ? body.prompt_max_chars
        : 0,
    max_skills:
      typeof body.max_skills === 'number' && Number.isFinite(body.max_skills)
        ? body.max_skills
        : 0,
    truncated: body.truncated === true,
    roots: roots.map(readSkillRoot).filter((root): root is SkillRoot => root !== null),
    skills: skills.map(readSkill).filter((skill): skill is SkillSummary => skill !== null),
  };
}

/**
 * `GET /api/commands?project=<id>` — the slash-command catalog.
 *
 * Built-ins plus every skill that can be typed as `/<id>`. The server resolves the
 * command from the message itself, so this list is the menu's content and nothing
 * more: a client that never calls it still gets the same commands.
 */
export async function listCommands(
  project?: string | null,
  options: CallOptions = {},
): Promise<CommandSpec[]> {
  const response = await apiRequest<CommandsResponse>(
    withQuery('/api/commands', { project: project ?? undefined }),
    options,
  );
  return Array.isArray(response.commands) ? response.commands : [];
}

/**
 * `GET /api/skills?project=<id>` — the switches, the folders discovery looked in
 * and every skill it found.
 */
export async function listSkills(
  project?: string | null,
  options: CallOptions = {},
): Promise<SkillListResponse> {
  const raw = await apiRequest<unknown>(
    withQuery('/api/skills', { project: project ?? undefined }),
    options,
  );
  return readSkillList(raw);
}

/** `GET /api/skills/:id?project=<id>` — the row plus the markdown body. */
export async function getSkill(
  id: string,
  project?: string | null,
  options: CallOptions = {},
): Promise<SkillDetail> {
  const raw = await apiRequest<unknown>(
    withQuery(`/api/skills/${encodeURIComponent(id)}`, { project: project ?? undefined }),
    options,
  );
  const row = readSkill(raw) ?? {
    id,
    name: id,
    description: '',
    source: 'custom' as const,
    path: null,
    enabled: true,
    custom: true,
  };
  const body = (typeof raw === 'object' && raw !== null ? raw : {}) as Record<string, unknown>;
  return {
    ...row,
    body: typeof body.body === 'string' ? body.body : null,
    body_truncated: body.body_truncated === true,
  };
}

/**
 * `POST /api/skills?project=<id>` — 201 with the new row, 409 when that id is
 * already taken (by a file-backed skill just as much as by a custom one).
 */
export async function createSkill(
  request: SkillCreateRequest,
  project?: string | null,
  options: CallOptions = {},
): Promise<SkillSummary | null> {
  const raw = await apiRequest<unknown>(withQuery('/api/skills', { project: project ?? undefined }), {
    ...options,
    method: 'POST',
    body: request,
  });
  return readSkill(raw);
}

/**
 * `PATCH /api/skills/:id?project=<id>` — the updated row.
 *
 * `{"enabled": false}` works for every skill; `name`/`description`/`body` answer
 * 400 for a file-backed one, naming the file.
 */
export async function updateSkill(
  id: string,
  request: SkillUpdateRequest,
  project?: string | null,
  options: CallOptions = {},
): Promise<SkillSummary | null> {
  const raw = await apiRequest<unknown>(
    withQuery(`/api/skills/${encodeURIComponent(id)}`, { project: project ?? undefined }),
    { ...options, method: 'PATCH', body: request },
  );
  return readSkill(raw);
}

/**
 * `DELETE /api/skills/:id?project=<id>` — 204 with no body.
 *
 * A file-backed skill answers 400 naming the file. `apiRequest` turns that body
 * into an `ApiError` whose `message` is the server's own sentence, and a missing
 * or non-JSON body still produces a message naming the route - so the caller can
 * always say *why* the delete was refused instead of silently doing nothing.
 */
export function deleteSkill(
  id: string,
  project?: string | null,
  options: CallOptions = {},
): Promise<void> {
  return apiRequest<void>(
    withQuery(`/api/skills/${encodeURIComponent(id)}`, { project: project ?? undefined }),
    { ...options, method: 'DELETE' },
  );
}

/**
 * `PATCH /api/config` with only the `skills` object.
 *
 * The response is the whole config, so the caller re-reads the stored values from
 * `config.skills`: the server clamps `prompt_max_chars` and `max_skills`, and an
 * input that kept the number someone typed would be lying about what is running.
 */
export function updateSkillsConfig(
  patch: SkillsConfigPatch,
  options: CallOptions = {},
): Promise<Config> {
  return apiRequest<Config>('/api/config', {
    ...options,
    method: 'PATCH',
    body: { skills: patch },
  });
}
