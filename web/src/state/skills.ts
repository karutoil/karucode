import { signal } from '@preact/signals';
import { toErrorMessage } from '../api/client';
import * as api from '../api/endpoints';
import type {
  SkillCreateRequest,
  SkillDetail,
  SkillListResponse,
  SkillSummary,
  SkillUpdateRequest,
  SkillsConfigPatch,
} from '../api/types';
import * as store from './store';

/**
 * The Skills tab's state, and the only place that talks to `/api/skills`.
 *
 * Everything a person can see about skills comes from one read:
 * `GET /api/skills` answers the six config switches, the folders discovery
 * looked in and every skill it found. The tab loads it once when it opens and
 * again on an explicit Refresh or after a mutation - it never polls, because
 * nothing here changes without somebody using this panel or dropping a file on
 * disk.
 *
 * Every failure keeps the server's own sentence in {@link skillsError}. A
 * silent no-op would be the worst outcome: the list would keep showing the old
 * value and the person would believe the change was saved.
 */
export const skillsData = signal<SkillListResponse | null>(null);
export const skillsLoading = signal(false);
/** The last failure, verbatim from the server when it sent one. */
export const skillsError = signal<string | null>(null);
/** True while a mutation is in flight; every action control is disabled. */
export const skillsBusy = signal(false);
/** The skill whose body the viewer/edit dialog is showing. */
export const skillBodyId = signal<string | null>(null);
export const skillBody = signal<SkillDetail | null>(null);
export const skillBodyLoading = signal(false);
/**
 * True after a save that changes *which folders are scanned*.
 *
 * `auto_detect` and `directories` decide the roots, and the roots only come back
 * from a fresh `GET /api/skills`. Rather than silently re-reading (and racing the
 * person's next keystroke) the tab says the roots are stale and offers Refresh.
 */
export const skillRootsStale = signal(false);

function projectId(): string | null {
  return store.currentProjectId.value;
}

/** Replace one row with what the server returned. Never merge field by field. */
function replaceSkill(next: SkillSummary | null): void {
  const data = skillsData.value;
  if (!data || !next) return;
  skillsData.value = {
    ...data,
    skills: data.skills.map((skill) => (skill.id === next.id ? next : skill)),
  };
}

/** Copy the six config fields out of a payload, dropping `undefined`. */
function configFields(source: Record<string, unknown>): SkillsConfigPatch {
  const fields: SkillsConfigPatch = {};
  if (typeof source.enabled === 'boolean') fields.enabled = source.enabled;
  if (typeof source.auto_detect === 'boolean') fields.auto_detect = source.auto_detect;
  if (Array.isArray(source.directories)) {
    fields.directories = source.directories.filter(
      (entry): entry is string => typeof entry === 'string',
    );
  }
  if (typeof source.prompt === 'boolean') fields.prompt = source.prompt;
  if (typeof source.prompt_max_chars === 'number' && Number.isFinite(source.prompt_max_chars)) {
    fields.prompt_max_chars = source.prompt_max_chars;
  }
  if (typeof source.max_skills === 'number' && Number.isFinite(source.max_skills)) {
    fields.max_skills = source.max_skills;
  }
  return fields;
}

/**
 * Adopt the values a `PATCH /api/config` response reports.
 *
 * The patch is only what was *asked* for. `config.skills` is what was stored -
 * the server clamps `prompt_max_chars` and `max_skills` - so the response wins
 * and the patch is only the fallback for a server that answers without the
 * section. Either way the inputs end up showing something the server said.
 */
function applyStoredConfig(patch: SkillsConfigPatch, stored: unknown): void {
  const data = skillsData.value;
  if (!data) return;
  const response = typeof stored === 'object' && stored !== null ? stored : {};
  skillsData.value = {
    ...data,
    ...configFields(patch as Record<string, unknown>),
    ...configFields(response as Record<string, unknown>),
  };
}

/** `GET /api/skills` — the whole panel, once. */
export async function loadSkills(): Promise<void> {
  skillsLoading.value = true;
  skillsError.value = null;
  try {
    skillsData.value = await api.listSkills(projectId());
    skillRootsStale.value = false;
  } catch (error) {
    skillsError.value = toErrorMessage(error);
  } finally {
    skillsLoading.value = false;
  }
}

/** The explicit Refresh: same read, but a press that does not look like a stuck panel. */
export async function refreshSkills(): Promise<void> {
  await loadSkills();
}

/**
 * `PATCH /api/config` with a `skills` object.
 *
 * Used by the master switch (immediate), by the options block's Save button and
 * by the directory editor's add/remove. Returns whether it was stored, so a
 * caller can keep a dialog or a draft open when it was not.
 */
export async function saveOptions(patch: SkillsConfigPatch): Promise<boolean> {
  skillsBusy.value = true;
  skillsError.value = null;
  try {
    const config = await api.updateSkillsConfig(patch);
    applyStoredConfig(patch, config?.skills);
    // The root list is what `auto_detect` and `directories` decide; the response
    // to a config patch does not carry it.
    if (patch.auto_detect !== undefined || patch.directories !== undefined) {
      skillRootsStale.value = true;
    }
    return true;
  } catch (error) {
    skillsError.value = toErrorMessage(error);
    return false;
  } finally {
    skillsBusy.value = false;
  }
}

/** `PATCH /api/skills/:id` with `{enabled}`; the row takes the server's answer. */
export async function toggleSkill(id: string, enabled: boolean): Promise<boolean> {
  skillsBusy.value = true;
  skillsError.value = null;
  try {
    replaceSkill(await api.updateSkill(id, { enabled }, projectId()));
    return true;
  } catch (error) {
    skillsError.value = toErrorMessage(error);
    return false;
  } finally {
    skillsBusy.value = false;
  }
}

/** `GET /api/skills/:id`, kept so the viewer and the editor share one read. */
export async function openSkillBody(id: string): Promise<void> {
  skillBodyId.value = id;
  skillBody.value = null;
  skillBodyLoading.value = true;
  skillsError.value = null;
  try {
    skillBody.value = await api.getSkill(id, projectId());
  } catch (error) {
    skillsError.value = toErrorMessage(error);
  } finally {
    skillBodyLoading.value = false;
  }
}

export function closeSkillBody(): void {
  skillBodyId.value = null;
  skillBody.value = null;
  skillBodyLoading.value = false;
}

/** `POST /api/skills`, then re-read: the new id may have been slugified. */
export async function addSkill(request: SkillCreateRequest): Promise<boolean> {
  skillsBusy.value = true;
  skillsError.value = null;
  try {
    await api.createSkill(request, projectId());
    await loadSkills();
    return true;
  } catch (error) {
    skillsError.value = toErrorMessage(error);
    return false;
  } finally {
    skillsBusy.value = false;
  }
}

/** `PATCH /api/skills/:id` with the text fields of a custom skill, then re-read. */
export async function editSkill(id: string, request: SkillUpdateRequest): Promise<boolean> {
  skillsBusy.value = true;
  skillsError.value = null;
  try {
    await api.updateSkill(id, request, projectId());
    await loadSkills();
    return true;
  } catch (error) {
    skillsError.value = toErrorMessage(error);
    return false;
  } finally {
    skillsBusy.value = false;
  }
}

/**
 * `DELETE /api/skills/:id`, then re-read.
 *
 * A 400 (a file-backed skill) is a *message*, not a hidden outcome: the server
 * names the file and says to turn the skill off instead, and that sentence is
 * what {@link skillsError} carries to the panel.
 */
export async function deleteSkill(id: string): Promise<boolean> {
  skillsBusy.value = true;
  skillsError.value = null;
  try {
    await api.deleteSkill(id, projectId());
    await loadSkills();
    return true;
  } catch (error) {
    skillsError.value = toErrorMessage(error);
    return false;
  } finally {
    skillsBusy.value = false;
  }
}

/** Test-only reset: signals are module state and would leak between cases. */
export function resetSkillsForTests(): void {
  skillsData.value = null;
  skillsLoading.value = false;
  skillsError.value = null;
  skillsBusy.value = false;
  skillBodyId.value = null;
  skillBody.value = null;
  skillBodyLoading.value = false;
  skillRootsStale.value = false;
}