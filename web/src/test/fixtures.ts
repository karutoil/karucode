import { dropWorkspaceImages } from '../lib/workspaceImages';
import type {
  AgentSummary,
  ApprovalRecord,
  Config,
  QueuedMessage,
  SessionDetail,
  SessionSummary,
  StoredMessage,
} from '../api/types';
import { setAuthToken } from '../api/client';
import * as actions from '../state/actions';
import { emptyChatState } from '../state/chat';
import { resetProjectIcons } from '../state/projectIcons';
import { resetPromptTokens } from '../state/promptTokens';
import * as store from '../state/store';
import * as workbench from '../state/workbench';

export const NOW = 1_710_000_100_000;

/** One queued message, as `GET /api/sessions/:id` reports it. */
export function queuedFixture(overrides: Partial<QueuedMessage> = {}): QueuedMessage {
  return {
    id: 'steer_1',
    kind: 'steer',
    content: 'also check the fixtures',
    draft: 'also check the fixtures',
    accepted_at: NOW - 1_000,
    ...overrides,
  };
}

/**
 * The same entry as it arrives on the wire when the draft is redundant.
 *
 * Rust declares `draft` as `Option<String>` with
 * `skip_serializing_if = "Option::is_none"` (`kc-core/src/queue.rs`), and the
 * `queued_messages` SSE frame serialises the raw struct, so a normal message with
 * no attachments carries **no** `draft` key at all. A fixture that always sets it
 * is what let the required-`draft` lie survive.
 */
export function withoutDraft(entry: QueuedMessage): Omit<QueuedMessage, 'draft'> {
  const { draft: _draft, ...rest } = entry;
  return rest;
}

export const configFixture: Config = {
  workspace: '/home/me/project',
  model: {
    provider: 'openai_compatible',
    base_url: 'http://127.0.0.1:4000/v1',
    model: 'gpt-4o-mini',
    temperature: 0.2,
    send_temperature: false,
    max_tokens: null,
    top_p: null,
    request_timeout_secs: 300,
    parallel_tool_calls: true,
    supports_images: false,
    api_key_set: true,
  },
  providers: [
    {
      id: 'default',
      name: 'Default',
      provider: 'openai_compatible',
      base_url: 'http://127.0.0.1:4000/v1',
      models: [{ id: 'gpt-4o-mini', name: 'GPT-4o mini', context_window: 128000 }],
      request_timeout_secs: 300,
      api_key_set: true,
      aggregator: false,
    },
  ],
  active: { provider: 'default', model: 'gpt-4o-mini' },
  limits: {
    max_iterations: 32,
    max_run_secs: 1800,
    max_tool_output_bytes: 24000,
    max_command_output_bytes: 32000,
    command_timeout_secs: 120,
    max_command_timeout_secs: 600,
    max_file_read_bytes: 200000,
    search_max_results: 200,
    context_max_chars: 240000,
    max_tool_calls_per_turn: 8,
    max_turn_retries: 2,
    turn_retry_delay_ms: 30000,
    parallel_tool_calls: true,
  },
  approvals: {
    mode: 'ask',
    allow_commands: ['cargo test'],
    auto_approve_paths: [],
    timeout_secs: 300,
  },
  subagents: {
    max_concurrent: 3,
    max_iterations: 16,
    allow_recursive: false,
    max_depth: 1,
    timeout_secs: 900,
    allow_background: true,
    max_background: 8,
    wait_timeout_secs: 600,
  },
  plugins: [],
  mcp_servers: [],
  notifications: {
    enabled: true,
    run_finished: true,
    needs_input: true,
    error: true,
    info: true,
    plugin: true,
    min_run_secs: 0,
    say_body: true,
    ephemeral_sessions: false,
    max_min_run_secs: 3600,
    kinds: ['run_finished', 'needs_input', 'error', 'info'],
  },
  settings_locked: false,
  // The version is the commit count, not a semver string.
  version: '107',
  updates: {
    check: true,
    check_secs: 21600,
    manifest_url: 'https://github.com/karutoil/karucode/releases/latest/download/updates.json',
    auto_apply: false,
    min_check_secs: 300,
    max_check_secs: 604800,
    default_manifest_url: 'https://github.com/karutoil/karucode/releases/latest/download/updates.json',
  },
};

export const summaryFixture: SessionSummary = {
  id: 's_ab12',
  title: 'Fix login bug',
  workspace: '/home/me/project',
  project_id: null,
  created_at: NOW - 3_600_000,
  updated_at: NOW - 120_000,
  message_count: 12,
  last_run_status: 'completed',
  running: false,
};

export const secondSummaryFixture: SessionSummary = {
  id: 's_cd34',
  title: 'Add workspace tree',
  workspace: '/home/me/project',
  project_id: null,
  created_at: NOW - 7_200_000,
  updated_at: NOW - 7_000_000,
  message_count: 3,
  last_run_status: 'failed',
  running: false,
};

export function storedMessage(partial: Partial<StoredMessage> & { id: number; role: StoredMessage['role'] }): StoredMessage {
  return {
    id: partial.id,
    session_id: partial.session_id ?? 's_ab12',
    run_id: partial.run_id ?? 'r_x',
    agent_id: partial.agent_id ?? null,
    role: partial.role,
    content: partial.content ?? '',
    tool_calls: partial.tool_calls ?? null,
    tool_call_id: partial.tool_call_id ?? null,
    name: partial.name ?? null,
    reasoning_content: partial.reasoning_content ?? null,
    created_at: partial.created_at ?? NOW - 60_000,
  };
}

export const approvalFixture: ApprovalRecord = {
  id: 'ap_1',
  session_id: 's_ab12',
  run_id: 'r_x',
  agent_id: null,
  kind: 'shell_command',
  title: 'Run cargo test',
  detail: 'cargo test',
  payload: { command: 'cargo test', cwd: '.' },
  status: 'pending',
  decision: null,
  created_at: NOW - 30_000,
  resolved_at: null,
};

export const detailFixture: SessionDetail = {
  session: {
    id: 's_ab12',
    title: 'Fix login bug',
    workspace: '/home/me/project',
    project_id: null,
    created_at: NOW - 3_600_000,
    updated_at: NOW - 120_000,
  },
  messages: [
    storedMessage({ id: 1, role: 'user', content: 'Find the login bug.', run_id: 'r_x', created_at: NOW - 90_000 }),
    storedMessage({
      id: 2,
      role: 'assistant',
      content: 'I will read the auth module.',
      run_id: 'r_x',
      created_at: NOW - 89_000,
      tool_calls: [{ id: 'call_1', name: 'read_file', arguments: '{"path":"src/auth.rs"}' }],
    }),
    storedMessage({
      id: 3,
      role: 'tool',
      content: 'fn login() { todo!() }',
      tool_call_id: 'call_1',
      name: 'read_file',
      created_at: NOW - 88_000,
    }),
    storedMessage({
      id: 4,
      role: 'assistant',
      content: 'The login handler is unfinished.',
      run_id: 'r_x',
      created_at: NOW - 87_000,
    }),
  ],
  runs: [
    {
      id: 'r_x',
      session_id: 's_ab12',
      status: 'completed',
      model: 'gpt-4o-mini',
      prompt: 'Find the login bug.',
      error: null,
      started_at: NOW - 90_000,
      finished_at: NOW - 86_000,
    },
  ],
  agents: [
    {
      id: 'a_1',
      session_id: 's_ab12',
      parent_run_id: 'r_x',
      parent_agent_id: null,
      name: 'reviewer',
      role: 'reviewer',
      task: 'Review the auth diff',
      status: 'completed',
      model: 'gpt-4o-mini',
      iterations: 3,
      summary: 'No issues found.',
      error: null,
      owned_paths: ['src/auth.rs'],
      created_at: NOW - 85_000,
      updated_at: NOW - 84_000,
      finished_at: NOW - 84_000,
      background: false,
      note: '',
      progress: null,
    },
  ],
  approvals: [approvalFixture],
  claims: [],
  running: false,
};

/**
 * One agent row exactly as `GET /api/agents` writes it on the wire.
 *
 * Not every `AgentRecord` field is always present: the Rust struct marks
 * `note` (`skip_serializing_if = "String::is_empty"`), `progress`,
 * `parent_agent_id`, `error` and `finished_at` (`skip_serializing_if =
 * Option::is_none`) as omittable, and a live server omitted `note` and
 * `progress` on all 77 rows of a run. A fixture that fills them with `''`/
 * `null` therefore tests a payload the server never sends — which is how a
 * blank-screen crash shipped with a green suite.
 *
 * Use this (plus `omitFields`) for anything that claims to model a server
 * response; keep the richer hand-written fixtures for component-level tests.
 */
export function wireAgentSummary(
  id: string,
  overrides: Record<string, unknown> = {},
): AgentSummary {
  return {
    id,
    session_id: 's_ab12',
    parent_run_id: 'r_1',
    name: id,
    role: 'reviewer',
    task: `review ${id}`,
    status: 'completed',
    model: 'gpt-4o-mini',
    iterations: 3,
    summary: '',
    owned_paths: [],
    created_at: NOW - 60_000,
    updated_at: NOW - 60_000,
    background: false,
    session_title: 'Fix login bug',
    workspace: '/home/me/project',
    project_id: null,
    // Deliberately absent here: note, progress, parent_agent_id, error,
    // finished_at — serde skips each of them when it is empty/None.
    ...overrides,
  } as unknown as AgentSummary;
}

/** Remove keys entirely, the way serde does when it skips a field. */
export function omitFields<T extends object>(row: T, ...keys: string[]): T {
  const copy = { ...row } as Record<string, unknown>;
  for (const key of keys) delete copy[key];
  return copy as T;
}

export function resetAppState(): void {
  workbench.closeContext();
  workbench.selectContextSection('now');
  actions.resetRuntimeForTests();
  resetProjectIcons();
  setAuthToken(null);
  store.writeLastSessionId(null);
  resetPromptTokens();
  // Cached workspace image blobs are module state as well: a test that resolved one
  // must not hand the next test a URL from a torn-down document.
  dropWorkspaceImages();
  store.boot.value = 'loading';
  store.bootError.value = null;
  store.health.value = null;
  store.config.value = null;
  // The update status is fetched once per page load, so a test that left one
  // behind would make the next one think an update was already known.
  store.updateStatus.value = null;
  store.settingsInitialTab.value = null;
  store.authError.value = null;
  // The sign-in and setup state is module state too: a test that signed in or
  // finished the wizard must not hand the next one a half-filled form.
  store.authStatus.value = null;
  store.authPending.value = false;
  store.onboarding.value = null;
  store.onboardingStep.value = 0;
  store.accessSettings.value = null;
  store.sessions.value = [];
  store.sessionsLoading.value = true;
  store.currentSessionId.value = null;
  // The draft is client-only state too: a test that started one must not hand
  // "New session" to the next test, nor a half-consumed handoff to the composer.
  store.draftSession.value = null;
  store.draftToSession.value = null;
  // The open session's model pin lives outside every list, so it has to be
  // cleared here too - otherwise a test would inherit the previous one's pin.
  store.openSessionModel.value = null;
  // The notification log backs the unread badges, the project counters and the
  // centre, so a test that leaks entries would leak counts into the next one.
  store.notifications.value = [];
  store.notificationCenterOpen.value = false;
  store.chat.value = emptyChatState(null);
  store.sessionHistory.value = null;
  store.agentHistory.value = null;
  store.chatLoading.value = false;
  store.chatError.value = null;
  store.connection.value = { api: 'unknown', stream: 'idle', lastError: null, checkedAt: null };
  store.railCollapsed.value = false;
  // The phone tier decides the composer's placeholder and whether model and
  // reasoning share one pill; a test that switched it must not leak a phone
  // layout into the next one.
  store.phoneViewport.value = false;
  store.narrowViewport.value = false;
  store.inspectorCollapsed.value = false;
  store.inspectorMode.value = 'docked';
  // The project dropdown's open mode is a stored browser preference too: a test
  // that switched it to click must not hand a click-only rail to the next one.
  store.projectMenuOpen.value = 'hover';
  store.mobileSheet.value = null;
  store.inspectorTab.value = 'workspace';
  store.settingsOpen.value = false;
  store.paletteOpen.value = false;
  store.rewindTarget.value = null;
  store.fileViewerPath.value = null;
  store.resolvingApprovals.value = {};
  store.stopping.value = false;
  store.agentsAll.value = [];
  store.agentsLoading.value = false;
  store.agentsError.value = null;
  store.agentScope.value = 'workspace';
  store.agentFilter.value = 'all';
  store.selectedAgentId.value = null;
  store.agentActivities.value = {};
  store.agentDetailLoading.value = false;
  store.agentDetailError.value = null;
  store.sessionInfo.value = null;
  store.sessionInfoFor.value = null;
  store.sessionInfoLoading.value = false;
  store.sessionInfoError.value = null;
  store.sessionInfoTick.value = 0;
  actions.resetPluginPanelData();
  store.composerFocusTick.value = 0;
  store.sentMessageTick.value = 0;
  store.composerDraft.value = null;
  store.composerHandoff.value = null;
  store.recallingQueued.value = {};
  store.resetToastTimers();
}

export function jsonResponse(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

export function errorResponse(code: string, message: string, status = 400): Response {
  return jsonResponse({ error: { code, message } }, status);
}