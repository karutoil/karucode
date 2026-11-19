import type { ApprovalPolicy } from './approvals';
import type { AccessConfig, AccessPatch } from './auth';
import type { ConfigMcpSummary } from './mcp';
import type { NotificationsConfig, NotificationsPatch } from './notifications';
import type { ConfigPluginSummary, PluginPanelGroup, PluginUiConfig } from './plugins';
import type { Project } from './projects';
import type {
  ActivePatch,
  ActiveSelection,
  ModelConfigPatch,
  ProviderProfile,
  ProvidersPatch,
  PublicModelConfig,
} from './providers';
import type { SkillsConfigPatch, SkillsConfigSummary } from './skills';
import type { UpdatesConfig, UpdatesPatch } from './updates';

export interface Health {
  status: string;
  /** The build number as a string (`"107"`). See `BuildInfo`. */
  version: string;
  /** The same number as an integer. Absent on a server that predates build numbers. */
  build?: number;
  /** Short commit sha this binary was built from. */
  commit?: string;
  /** Commit date, RFC 3339. */
  commit_date?: string;
  /** Built from a working tree with uncommitted changes. */
  dirty?: boolean;
  uptime_secs: number;
  /**
   * The server's implicit workspace, or `null` on a fresh install: nothing is
   * chosen until the operator creates a project in the web UI.
   */
  workspace: string | null;
  /** Every project the server can serve, newest counts included. */
  projects: Project[];
  auth_required: boolean;
  provider_configured: boolean;
  db: string;
}

export interface Limits {
  /** Model turns for one run. `0` - the default - means no ceiling. */
  max_iterations: number;
  max_run_secs: number;
  max_tool_output_bytes: number;
  max_command_output_bytes: number;
  command_timeout_secs: number;
  max_command_timeout_secs: number;
  max_file_read_bytes: number;
  search_max_results: number;
  context_max_chars: number;
  max_tool_calls_per_turn: number;
  /**
   * How many times a turn the provider gave up on is asked again, after a wait.
   * `0` disables the layer. Each retry is a fresh generation.
   */
  max_turn_retries: number;
  /** Wait before the first turn retry, in ms; doubles per retry, capped 8x. */
  turn_retry_delay_ms: number;
  /**
   * Run a turn's independent tool calls at the same time. Off runs every call
   * one at a time, in order. Not the same key as `model.parallel_tool_calls`,
   * which asks the endpoint for several calls in one turn.
   */
  parallel_tool_calls: boolean;
}

export interface SubagentConfig {
  /** Subagent loops allowed to run at once, across the whole install. */
  max_concurrent: number;
  /** Model turns for one subagent. `0` - the default - means no ceiling. */
  max_iterations: number;
  allow_recursive: boolean;
  max_depth: number;
  timeout_secs: number;
  /** Whether a spawn may detach and keep working after its tool call returns. */
  allow_background: boolean;
  /** Ceiling on live detached agents per session. */
  max_background: number;
  /** Default bound on `wait_for_agents`, in seconds. */
  wait_timeout_secs: number;
  /**
   * Give each subagent its own git worktree instead of the parent's live tree.
   * Off by default; a child's changes come back on a `karucode/subagent/<id>`
   * branch rather than landing in this workspace.
   */
  worktrees: boolean;
}

export interface Stats {
  uptime_secs: number;
  rss_kb: number;
  sessions: number;
  active_runs: number;
  events_emitted: number;
  db_path: string;
}

export interface Config {
  workspace: string;
  /** Live/effective model connection (mirrors the active provider profile). */
  model: PublicModelConfig;
  /** Named endpoints and their model libraries. */
  providers: ProviderProfile[];
  active: ActiveSelection;
  limits: Limits;
  approvals: ApprovalPolicy;
  subagents: SubagentConfig;
  /**
   * Server-side notification routing. Absent on an older server, which emits no
   * `notification` events at all.
   */
  notifications?: NotificationsConfig;
  plugins: ConfigPluginSummary[];
  /**
   * Inspector panels contributed by ready plugins, grouped per plugin.
   * Absent on an older server, which simply means "no plugin tabs".
   */
  plugin_panels?: PluginPanelGroup[];
  /**
   * What plugins contribute to the app's own UI: buttons in named slots, palette
   * commands, and built-in controls they asked to hide.
   *
   * Absent on an older server, which simply means "no plugin controls". The
   * frontend re-validates every entry (`components/uiContributions.ts`) instead
   * of trusting the shape.
   */
  plugin_ui?: PluginUiConfig;
  mcp_servers: ConfigMcpSummary[];
  /**
   * Whether the server re-reads plugin sources on a timer (`plugins_watch`), and
   * how often. Absent on an older server, which never hot-reloads on its own.
   */
  plugins_watch?: boolean;
  plugins_watch_secs?: number;
  /**
   * Whether the server re-reads `[[mcp_servers]]` on a timer (`mcp_watch`), and
   * how often. Absent on an older server, which only reloads on demand.
   */
  mcp_watch?: boolean;
  mcp_watch_secs?: number;
  /**
   * `[updates]`: whether the server looks for a newer build. Absent on an older
   * server, which never checks (and has no update routes at all).
   */
  updates?: UpdatesConfig;
  settings_locked: boolean;
  /** Provider ids this server can actually build. Drives the settings selector. */
  available_providers?: string[];
  /**
   * The web-UI access policy. Absent on an older server, which has no sign-in at
   * all beyond the bearer token.
   */
  access?: AccessConfig;
  /**
   * The `[skills]` section. Absent on an older server, which has no skills
   * feature at all - Settings → Skills reads `GET /api/skills` instead, so an
   * absent field here costs nothing.
   */
  skills?: SkillsConfigSummary;
  version: string;
}

export interface ConfigPatch {
  model?: ModelConfigPatch;
  providers?: ProvidersPatch;
  active?: ActivePatch;
  limits?: Partial<Limits>;
  approvals?: Partial<ApprovalPolicy>;
  subagents?: Partial<SubagentConfig>;
  notifications?: NotificationsPatch;
  access?: AccessPatch;
  updates?: UpdatesPatch;
  /**
   * The `[skills]` section: any subset of its six fields.
   *
   * `skills.custom` and `skills.disabled` are deliberately **not** here - they
   * are managed by the `/api/skills` routes, not by a config patch.
   */
  skills?: SkillsConfigPatch;
}
