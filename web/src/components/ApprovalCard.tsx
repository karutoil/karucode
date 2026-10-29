import type { JSX } from 'preact';
import type { JsonValue } from '../api/types';
import type { ApprovalView } from '../state/chat';
import { AlertIcon } from './icons';
import { DiffView } from './DiffView';

export interface ApprovalCardProps {
  approval: ApprovalView;
  resolving: boolean;
  onDecide: (decision: 'allow_once' | 'allow_session' | 'deny') => void;
}

const KIND_LABELS: Record<string, string> = {
  shell_command: 'Shell command',
  file_write: 'File write',
  file_edit: 'File edit',
  delete_file: 'Delete file',
  network_tool: 'Network tool',
  plugin: 'Plugin tool',
  mcp: 'MCP tool',
  other: 'Action',
};

const DECISION_LABELS: Record<string, string> = {
  allow_once: 'Allowed once',
  allow_session: 'Allowed for session',
  deny: 'Denied',
  expired: 'Expired',
};

function payloadRecord(payload: JsonValue): Record<string, JsonValue> | null {
  if (typeof payload === 'object' && payload !== null && !Array.isArray(payload)) return payload;
  return null;
}

function stringField(record: Record<string, JsonValue> | null, key: string): string {
  const value = record?.[key];
  return typeof value === 'string' ? value : '';
}

export function ApprovalCard({ approval, resolving, onDecide }: ApprovalCardProps): JSX.Element {
  const pending = approval.status === 'pending';
  const payload = payloadRecord(approval.payload);
  const command = stringField(payload, 'command');
  const path = stringField(payload, 'path');
  const diff = stringField(payload, 'diff');
  const cwd = stringField(payload, 'cwd');
  const timeout = payload?.timeout_secs;
  const hasTimeout = typeof timeout === 'number';
  const titleId = `approval-title-${approval.id}`;

  return (
    <article class="approval-card" aria-labelledby={titleId} aria-busy={resolving}>
      <header class="approval-head">
        <span class="approval-kicker">
          <AlertIcon size={14} />
          Approval required
          <span class="approval-kind">{KIND_LABELS[approval.kind] ?? approval.kind}</span>
        </span>
        <h3 id={titleId} class="approval-title">
          {approval.title}
        </h3>
        {approval.agent_id ? (
          <p class="approval-agent">Requested by subagent {approval.agent_id}</p>
        ) : null}
      </header>

      {command ? (
        <pre class="approval-command">
          <span class="approval-command-prompt" aria-hidden="true">
            $
          </span>{' '}
          {command}
        </pre>
      ) : null}

      {cwd && cwd !== '.' ? (
        <p class="approval-meta">
          <span class="approval-meta-label">working directory</span> <code>{cwd}</code>
        </p>
      ) : null}

      {path ? (
        <p class="approval-meta">
          <span class="approval-meta-label">path</span> <code>{path}</code>
        </p>
      ) : null}

      {hasTimeout ? (
        <p class="approval-meta">
          <span class="approval-meta-label">timeout</span> <code>{timeout}s</code>
        </p>
      ) : null}

      {approval.detail && !command && !path && !diff ? (
        <p class="approval-detail">{approval.detail}</p>
      ) : null}

      {diff ? (
        <div class="approval-diff">
          <DiffView diff={diff} path={path || undefined} showHeader={!path} />
        </div>
      ) : null}

      {!command && !path && !diff && payload ? (
        <pre class="approval-payload">{JSON.stringify(payload, null, 2)}</pre>
      ) : null}

      <p class="approval-warning">
        {approval.kind === 'shell_command' || approval.kind === 'network_tool'
          ? 'Shell commands and network tools are not sandboxed. Read the command before allowing.'
          : approval.kind === 'plugin' || approval.kind === 'mcp'
            ? 'Plugins and MCP tools run with the server process permissions. Only allow tools you trust.'
            : 'Review the path and payload before allowing.'}
      </p>

      <div class="approval-actions">
        {pending ? (
          <>
            <button
              type="button"
              class="button button-primary"
              disabled={resolving}
              onClick={() => onDecide('allow_once')}
            >
              Allow once
            </button>
            <button
              type="button"
              class="button"
              disabled={resolving}
              onClick={() => onDecide('allow_session')}
            >
              Allow for session
            </button>
            <button
              type="button"
              class="button button-danger-ghost"
              disabled={resolving}
              onClick={() => onDecide('deny')}
            >
              Deny
            </button>
            <span class="approval-hint" role="status">
              {resolving ? 'Sending decision…' : 'The run is waiting for this decision.'}
            </span>
          </>
        ) : (
          <span
            class={`approval-result ${approval.status === 'expired' || approval.decision === 'deny' ? 'is-negative' : 'is-positive'}`}
            role="status"
          >
            {approval.status === 'expired' ? 'Expired' : DECISION_LABELS[approval.decision ?? ''] ?? approval.status}
          </span>
        )}
      </div>
    </article>
  );
}