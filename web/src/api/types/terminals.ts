/**
 * One browser terminal.
 *
 * `id`, `title`, `cwd` and `position` are the *durable* half, stored in SQLite:
 * they come back in every browser and after every server restart. `running`,
 * `pid` and `attached` are the *process* half and describe the shell this
 * server has right now - a terminal with `running: false` is a tab that will
 * start a fresh shell when opened.
 */
export interface Terminal {
  id: string;
  title: string;
  /** Absolute directory the shell was started in. */
  cwd: string;
  /** The project it was created in, as a label only. */
  project: string | null;
  /** Sort key for the tab strip; ties break on `created_at`. */
  position: number;
  created_at: number;
  updated_at: number;
  last_used_at: number;
  /** A live shell exists for this tab right now. */
  running: boolean;
  pid: number;
  /** How many browsers are attached (0 or 1 in practice). */
  attached: number;
}

export interface TerminalList {
  /** False when the server has the terminal disabled; the tab is hidden then. */
  enabled: boolean;
  terminals: Terminal[];
  /** `terminals.max_terminals`, so the UI can disable **New** at the cap. */
  max_terminals?: number;
  /** The resolved shell program, shown as a hint. */
  shell?: string;
  scrollback_bytes?: number;
}

export interface TerminalCreateRequest {
  title?: string;
  cwd?: string;
  project?: string | null;
  cols?: number;
  rows?: number;
}

/**
 * A control frame from the terminal websocket. Binary frames are raw PTY bytes;
 * only text frames are JSON, so a program printing arbitrary bytes cannot forge
 * one of these.
 */
export type TerminalControlFrame =
  | { type: 'ready'; replay: boolean; replay_bytes: number; pid: number; running: boolean }
  | { type: 'exit'; code: number | null }
  | { type: 'lagged'; missed: number }
  | { type: 'detached' }
  | { type: 'error'; message: string }
  | { type: 'pong' };
