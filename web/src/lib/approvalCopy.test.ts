import { describe, expect, it } from 'vitest';
import type { ApprovalMode } from '../api/types';
import { configFixture } from '../test/fixtures';
import { describeApprovals } from './approvalCopy';

const descriptions: Record<ApprovalMode, { label: string; title: string }> = {
  ask: {
    label: 'ask',
    title: 'Every mutating tool and shell command asks for approval. 1 allowlisted command, 1 auto-approved path. Change this in Settings → Approvals.',
  },
  ask_commands: {
    label: 'commands only',
    title: 'File writes inside the workspace are allowed; shell commands still ask. 1 allowlisted command, 1 auto-approved path. Change this in Settings → Approvals.',
  },
  auto: {
    label: 'auto',
    title: 'No approval prompts: every tool call is allowed and recorded. 1 allowlisted command, 1 auto-approved path. Change this in Settings → Approvals.',
  },
  read_only: {
    label: 'read only',
    title: 'Only read-only tools are allowed; writes and commands are refused. 1 allowlisted command, 1 auto-approved path. Change this in Settings → Approvals.',
  },
};

describe('describeApprovals', () => {
  it('keeps the Composer mode labels and detailed titles exact', () => {
    for (const mode of Object.keys(descriptions) as ApprovalMode[]) {
      const config = {
        ...configFixture,
        approvals: {
          ...configFixture.approvals,
          mode,
          allow_commands: ['cargo test'],
          auto_approve_paths: ['src'],
        },
      };
      expect(describeApprovals(config)).toEqual(descriptions[mode]);
    }
  });

  it('uses singular/plural allowlist counts only for entries that exist', () => {
    const config = {
      ...configFixture,
      approvals: {
        ...configFixture.approvals,
        allow_commands: ['one', 'two'],
        auto_approve_paths: [],
      },
    };
    expect(describeApprovals(config)?.title).toBe(
      'Every mutating tool and shell command asks for approval. 2 allowlisted commands. Change this in Settings → Approvals.',
    );
  });

  it('keeps unknown configuration unknown', () => {
    expect(describeApprovals(null)).toBeNull();
  });
});
