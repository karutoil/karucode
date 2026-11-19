import type { JsonValue } from './common';

export type ApprovalMode = 'ask' | 'ask_commands' | 'auto' | 'read_only';

export interface ApprovalPolicy {
  mode: ApprovalMode;
  allow_commands: string[];
  auto_approve_paths: string[];
  timeout_secs: number;
}

export type ApprovalKind =
  | 'shell_command'
  | 'file_write'
  | 'file_edit'
  | 'delete_file'
  | 'network_tool'
  | 'plugin'
  | 'mcp'
  | 'other';

export type ApprovalStatus = 'pending' | 'resolved' | 'expired';
export type ApprovalDecision = 'allow_once' | 'allow_session' | 'deny' | 'expired';

/** Approval request as delivered by an `approval_requested` event. */
export interface ApprovalRequest {
  id: string;
  kind: ApprovalKind;
  title: string;
  detail: string;
  payload: JsonValue;
  session_id: string;
  run_id: string;
  agent_id?: string | null;
}

export interface ApprovalRecord {
  id: string;
  session_id: string;
  run_id: string;
  /** Absent when a top-level run raised the approval (`skip_serializing_if`). */
  agent_id?: string | null;
  kind: ApprovalKind;
  title: string;
  detail: string;
  payload: JsonValue;
  status: ApprovalStatus;
  /** Absent while pending. Read through `??`/truthiness everywhere. */
  decision?: ApprovalDecision | null;
  created_at: number;
  /** Absent while pending. Nothing in the UI reads it; safe either way. */
  resolved_at?: number | null;
}
