// ---------------------------------------------------------------------------
// Skills (`[skills]`, `GET /api/skills` and friends)
// ---------------------------------------------------------------------------

/**
 * The `[skills]` section, as `GET /api/config` and `GET /api/skills` report it.
 *
 * These six are the whole install-level surface: everything else about skills is
 * per-skill. `directories` holds only the extra folders a person added; the
 * conventional workspace and user folders are always scanned when `auto_detect`
 * is on.
 */
export interface SkillsConfigSummary {
  enabled: boolean;
  auto_detect: boolean;
  directories: string[];
  prompt: boolean;
  /**
   * How many characters of the skill list may reach the system prompt. The
   * server clamps it, so a caller must re-read the stored value from the
   * response rather than echo what was typed.
   */
  prompt_max_chars: number;
  /** How many skills the prompt block lists. Clamped by the server too. */
  max_skills: number;
}

/**
 * Where a skill came from, exactly as the API spells it.
 *
 * `kc_core::skill::SkillSource` is `#[serde(rename_all = "kebab-case")]`, so the
 * `source` field on the wire is one of these four strings. They are *not* the
 * human labels: the prompt and the `skill` tool use `SkillSource::label()`, which
 * says `configured` for `config` and `added` for `custom`. Anything that shows
 * the source to a person maps the value (`SkillsSettings.tsx`); anything that keys off
 * the wire (`data-source`) mirrors it verbatim.
 */
export type SkillSource = 'workspace' | 'user' | 'config' | 'custom';

/** One row of `GET /api/skills`. */
export interface SkillSummary {
  id: string;
  name: string;
  /** One line: when to use it. The prompt shows exactly this. */
  description: string;
  source: SkillSource;
  /** The file it was read from; absent for a skill that only lives in config. */
  path?: string | null;
  enabled: boolean;
  /** True for a skill added through the API (the only kind that can be deleted). */
  custom?: boolean;
}

/** `GET /api/skills/:id`: the row plus its markdown, frontmatter stripped. */
export interface SkillDetail extends SkillSummary {
  body?: string | null;
  /** True when the server cut the body at its own cap. */
  body_truncated?: boolean;
}

/**
 * One folder discovery looked in.
 *
 * `exists: false` is what makes "auto-detect found nothing" explainable: the
 * folder is where it was expected to be, and it is not there.
 */
export interface SkillRoot {
  path: string;
  source: SkillSource;
  exists: boolean;
  /** True for an entry that came from `skills.directories`. */
  configured?: boolean;
  /** How many skills this folder contributed after shadowing. */
  count: number;
}

export interface SkillListResponse extends SkillsConfigSummary {
  /** True when `max_skills` cut the list short. */
  truncated?: boolean;
  roots: SkillRoot[];
  skills: SkillSummary[];
}

/** `PATCH /api/config`'s optional `skills` object: any subset of the six. */
export interface SkillsConfigPatch {
  enabled?: boolean;
  auto_detect?: boolean;
  directories?: string[];
  prompt?: boolean;
  prompt_max_chars?: number;
  max_skills?: number;
}

/** `POST /api/skills`. `id`/`name` default to each other; `body` is required. */
export interface SkillCreateRequest {
  id?: string;
  name?: string;
  description: string;
  body: string;
}

/**
 * `PATCH /api/skills/:id`.
 *
 * `enabled` works for every skill. `name`/`description`/`body` work only for a
 * custom one: the server answers 400 for a file-backed skill, naming the file.
 */
export interface SkillUpdateRequest {
  enabled?: boolean;
  name?: string;
  description?: string;
  body?: string;
}

/** The menu group a slash command belongs to (`GET /api/commands`). */
export type CommandCategory = 'session' | 'context' | 'extend' | 'inspect' | 'meta';

/**
 * One row of `GET /api/commands`: a slash command the session understands.
 *
 * The catalog is built-ins plus every skill that can be typed as `/<id>`, so the
 * same shape covers both. `insert` is exactly what the composer writes into the
 * box when the row is picked (a command that takes arguments brings its trailing
 * space), which keeps the client from re-deriving the usage string from `args`.
 */
export interface CommandSpec {
  /** Without the slash. */
  name: string;
  title: string;
  summary: string;
  /** The argument placeholder, or `''` for a command that takes none. */
  args: string;
  category: CommandCategory;
  source: 'builtin' | 'skill';
  insert: string;
}

export interface CommandsResponse {
  commands: CommandSpec[];
}
