/** Why a human is being interrupted. Mirrors `kc_core::notify::NotifyKind`. */
export type NotifyKind = 'run_finished' | 'needs_input' | 'error' | 'info';

/**
 * `config.notifications` from `GET /api/config`.
 *
 * The switches are server-side on purpose: `enabled = false` means the server sends
 * no `notification` event at all, so a client cannot accidentally ignore it. The
 * settings screen edits these through `PATCH /api/config`.
 */
export interface NotificationsConfig {
  enabled: boolean;
  run_finished: boolean;
  needs_input: boolean;
  error: boolean;
  info: boolean;
  /** Whether plugins may raise notifications at all. */
  plugin: boolean;
  /** Skip notifications for runs shorter than this. */
  min_run_secs: number;
  /** Include the first line of the answer as the notification body. */
  say_body: boolean;
  /** Whether temporary (ephemeral) sessions notify at all. Off by default. */
  ephemeral_sessions: boolean;
  /** The documented ceiling, sent by the server so the UI need not hard-code it. */
  max_min_run_secs: number;
  /** The kinds this server can emit. */
  kinds: NotifyKind[];
}

/** `[notifications]` section of `PATCH /api/config`. */
export interface NotificationsPatch {
  enabled?: boolean;
  run_finished?: boolean;
  needs_input?: boolean;
  error?: boolean;
  info?: boolean;
  plugin?: boolean;
  min_run_secs?: number;
  say_body?: boolean;
  ephemeral_sessions?: boolean;
}

/**
 * One notification from `GET /api/events` (`type: "notification"`).
 *
 * Everything is plain text, capped by the server, and `source` is either
 * `"runtime"` or the name of the plugin that raised it. A client that does not
 * recognise a `kind` should render it as `info` rather than dropping it: the server
 * may learn kinds before the browser does.
 */
export interface NotifyPayload {
  /** Stable per source moment (`run:<id>`, `approval:<id>`, `plugin:<name>:<title>`). */
  id: string;
  kind: NotifyKind;
  title: string;
  body: string;
  session_id: string;
  /** The session's title at emit time, so the notification is readable on its own. */
  session_title: string;
  /**
   * The project the session belongs to, when the server could read it.
   *
   * Absent on a server that predates the field, and for a notice that names no
   * session at all. This is what lets the rail count unread news per project
   * without knowing which project owns a session it has never listed.
   */
  project_id?: string;
  /** `runtime`, or the plugin's name. */
  source: string;
  /** A cue override; absent means "the kind's default". */
  sound?: string | null;
  /** Keep the desktop notification until it is dismissed. */
  sticky: boolean;
}

export interface Toast {
  id: number;
  tone: 'info' | 'success' | 'error';
  message: string;
  /**
   * A notification about a session, rather than transient feedback. Such a toast
   * offers "Open" instead of only a dismiss button, and is what the unread badge is
   * cleared by.
   */
  sessionId?: string;
}
