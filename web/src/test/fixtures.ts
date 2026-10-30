import type {
  ApprovalRecord,
  Config,
  SessionDetail,
  SessionSummary,
  StoredMessage,
} from '../api/types';
import { setAuthToken } from '../api/client';
import * as actions from '../state/actions';
import { emptyChatState } from '../state/chat';
import { resetProjectIcons } from '../state/projectIcons';
import * as store from '../state/store';

export const NOW = 1_710_000_100_000;

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
    },
  ],
  active: { provider: 'default', model: 'gpt-4o-mini' },
  limits: {
    max_iterations: 32,
    max_run_secs: 1800,
    auto_continue: true,
    max_continues: 50,
    max_tool_output_bytes: 24000,
    max_command_output_bytes: 32000,
    command_timeout_secs: 120,
    max_command_timeout_secs: 600,
    max_file_read_bytes: 200000,
    search_max_results: 200,
    context_max_chars: 240000,
    max_tool_calls_per_turn: 8,
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
    max_continues: 0,
    allow_recursive: false,
    max_depth: 1,
    timeout_secs: 900,
  },
  plugins: [],
  mcp_servers: [],
  settings_locked: false,
  version: '0.1.0',
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
    },
  ],
  approvals: [approvalFixture],
  claims: [],
  running: false,
};

export function resetAppState(): void {
  actions.resetRuntimeForTests();
  resetProjectIcons();
  setAuthToken(null);
  store.writeLastSessionId(null);
  store.boot.value = 'loading';
  store.bootError.value = null;
  store.health.value = null;
  store.config.value = null;
  store.authError.value = null;
  store.sessions.value = [];
  store.sessionsLoading.value = true;
  store.currentSessionId.value = null;
  store.chat.value = emptyChatState(null);
  store.chatLoading.value = false;
  store.chatError.value = null;
  store.connection.value = { api: 'unknown', stream: 'idle', lastError: null, checkedAt: null };
  store.railCollapsed.value = false;
  store.inspectorCollapsed.value = false;
  store.mobileSheet.value = null;
  store.inspectorTab.value = 'workspace';
  store.settingsOpen.value = false;
  store.paletteOpen.value = false;
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
  actions.resetPluginPanelData();
  store.composerFocusTick.value = 0;
  store.sentMessageTick.value = 0;
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