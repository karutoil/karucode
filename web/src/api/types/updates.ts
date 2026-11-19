// ---------------------------------------------------------------------------
// Versioning and updates (`GET /api/update`)
// ---------------------------------------------------------------------------

/**
 * Which build this is.
 *
 * The version is the git commit count — build 107 is the 107th commit — because
 * karucode has no release trains: every commit on `main` is a release. Never
 * parse `version` as a float; use `build` to compare.
 */
export interface BuildInfo {
  /** The build number as a string, exactly as `--version` prints it. */
  version: string;
  build: number;
  commit: string;
  commit_date: string;
  /** The binary was built from a tree with uncommitted changes. */
  dirty: boolean;
  /** Release channel label (`stable` unless the build said otherwise). */
  channel: string;
  /** Target triple the binary was built for. */
  target: string;
}

/** How the running binary is installed, and whether this process may replace it. */
export interface UpdateInstallInfo {
  kind: 'managed' | 'source' | 'container';
  path: string;
  /** `false` on a source tree, a container, or a binary owned by another user. */
  writable: boolean;
  /** One sentence saying what an update would do here. */
  detail: string;
}

/** State of update checking itself. */
export interface UpdateCheckInfo {
  enabled: boolean;
  manifest_url: string | null;
  interval_secs: number;
  /** Unix milliseconds of the last attempt, successful or not. */
  checked_at: number | null;
  /** Why the last attempt failed; cleared by a successful one. */
  error: string | null;
}

/** The download for this target that `latest` names. */
export interface UpdateArtifactInfo {
  url: string;
  sha256: string;
  size: number | null;
  tarball: string | null;
}

/** The newest release the last check found. */
export interface UpdateLatest {
  build: number;
  version: string;
  commit: string | null;
  commit_date: string | null;
  released_at: string | null;
  /** Release notes, plain text, capped by the server. */
  notes: string | null;
  /** The human-readable release page. */
  url: string | null;
  artifact: UpdateArtifactInfo | null;
  /** `false` means "a newer build exists, but not for this platform". */
  has_target_for_this_build: boolean;
}

/** A verified download waiting to be installed. */
export interface StagedUpdate {
  build: number;
  version: string;
  commit: string | null;
  path: string;
  sha256: string;
  size: number;
  /** Unix milliseconds. */
  staged_at: number;
}

/** The last build this install put in place. */
export interface AppliedUpdate {
  from: string;
  to: string;
  path: string;
  /** Unix milliseconds. */
  applied_at: number;
}

/** Whether a restart is even possible, and the command that installs by hand. */
export interface UpdateRestartInfo {
  /** A supervisor (systemd) would start the new binary after an exit. */
  supervised: boolean;
  /** e.g. `sudo /usr/local/bin/karucode update --apply --data-dir /var/lib/karucode`. */
  apply_command: string;
}

/** `GET /api/update` — everything the Updates panel draws. */
export interface UpdateStatus {
  current: BuildInfo;
  install: UpdateInstallInfo;
  check: UpdateCheckInfo;
  latest: UpdateLatest | null;
  update_available: boolean;
  phase: 'idle' | 'checking' | 'downloading' | 'applying';
  staged: StagedUpdate | null;
  last_applied: AppliedUpdate | null;
  restart: UpdateRestartInfo;
}

/** What an apply did. */
export interface UpdateApplyResult {
  installed: boolean;
  /** The running process still holds the old binary: restart to run the new one. */
  restart_required: boolean;
  /** The server asked its supervisor to restart it. */
  restart_requested: boolean;
  from: string;
  to: string;
  path: string;
  /** This process could not write the binary; run `command` with privilege. */
  needs_privilege: boolean;
  command: string | null;
  message: string;
}

/** `POST /api/update/apply` */
export interface UpdateApplyResponse {
  update: UpdateStatus;
  result: UpdateApplyResult;
}

/** `[updates]` as `GET /api/config` reports it. */
export interface UpdatesConfig {
  check: boolean;
  /** Clamped by the server to `min_check_secs..=max_check_secs`. */
  check_secs: number;
  manifest_url: string;
  auto_apply: boolean;
  /** The documented bounds, so the form validates with the server's numbers. */
  min_check_secs: number;
  max_check_secs: number;
  default_manifest_url: string;
}

/** `[updates]` section of `PATCH /api/config`. */
export interface UpdatesPatch {
  check?: boolean;
  check_secs?: number;
  manifest_url?: string;
  auto_apply?: boolean;
}
