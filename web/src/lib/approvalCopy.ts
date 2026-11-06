import type { ApprovalMode, Config } from '../api/types';

/** Short footnote label for the active approval policy. */
const APPROVAL_FOOT_LABEL: Record<ApprovalMode, string> = {
  ask: 'ask',
  ask_commands: 'commands only',
  auto: 'auto',
  read_only: 'read only',
};

const APPROVAL_FOOT_TITLE: Record<ApprovalMode, string> = {
  ask: 'Every mutating tool and shell command asks for approval.',
  ask_commands: 'File writes inside the workspace are allowed; shell commands still ask.',
  auto: 'No approval prompts: every tool call is allowed and recorded.',
  read_only: 'Only read-only tools are allowed; writes and commands are refused.',
};

/**
 * The Composer’s right-hand policy summary, shared with Context’s Access panel.
 * Labels, explanations and allowlist counts intentionally remain byte-identical
 * to the existing Composer copy; Settings remains the only policy editor.
 */
export function describeApprovals(config: Config | null): { label: string; title: string } | null {
  const policy = config?.approvals;
  if (!policy) return null;
  const counts: string[] = [];
  const commands = policy.allow_commands.length;
  const paths = policy.auto_approve_paths.length;
  if (commands > 0) counts.push(`${commands} allowlisted command${commands === 1 ? '' : 's'}`);
  if (paths > 0) counts.push(`${paths} auto-approved path${paths === 1 ? '' : 's'}`);
  return {
    label: APPROVAL_FOOT_LABEL[policy.mode] ?? policy.mode,
    title: [
      APPROVAL_FOOT_TITLE[policy.mode] ?? `Approval mode: ${policy.mode}.`,
      counts.length > 0 ? `${counts.join(', ')}.` : '',
      'Change this in Settings → Approvals.',
    ]
      .filter(Boolean)
      .join(' '),
  };
}
