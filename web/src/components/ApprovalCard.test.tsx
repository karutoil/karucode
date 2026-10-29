import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { ApprovalCard } from './ApprovalCard';
import type { ApprovalView } from '../state/chat';
import { approvalFixture, NOW } from '../test/fixtures';

function view(overrides: Partial<ApprovalView> = {}): ApprovalView {
  return {
    id: approvalFixture.id,
    kind: approvalFixture.kind,
    title: approvalFixture.title,
    detail: approvalFixture.detail,
    payload: approvalFixture.payload,
    session_id: approvalFixture.session_id,
    run_id: approvalFixture.run_id,
    agent_id: null,
    status: 'pending',
    decision: null,
    created_at: NOW,
    ...overrides,
  };
}

describe('ApprovalCard', () => {
  it('renders the command payload and offers all three decisions', () => {
    render(<ApprovalCard approval={view()} resolving={false} onDecide={vi.fn()} />);
    expect(screen.getByText('Run cargo test')).not.toBeNull();
    expect(screen.getByText('cargo test')).not.toBeNull();
    expect(screen.getByText(/not sandboxed/i)).not.toBeNull();

    expect(screen.getByRole('button', { name: 'Allow once' })).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Allow for session' })).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Deny' })).not.toBeNull();
  });

  it('reports each decision with the approval id handled by the parent', () => {
    const onDecide = vi.fn();
    render(<ApprovalCard approval={view()} resolving={false} onDecide={onDecide} />);
    fireEvent.click(screen.getByRole('button', { name: 'Allow once' }));
    fireEvent.click(screen.getByRole('button', { name: 'Allow for session' }));
    fireEvent.click(screen.getByRole('button', { name: 'Deny' }));
    expect(onDecide.mock.calls).toEqual([['allow_once'], ['allow_session'], ['deny']]);
  });

  it('disables decisions and says it is sending while resolving', () => {
    render(<ApprovalCard approval={view()} resolving onDecide={vi.fn()} />);
    expect((screen.getByRole('button', { name: 'Allow once' }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole('button', { name: 'Deny' }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText('Sending decision…')).not.toBeNull();
    expect(screen.getByRole('article').getAttribute('aria-busy')).toBe('true');
  });

  it('shows the resolved decision instead of buttons', () => {
    render(
      <ApprovalCard
        approval={view({ status: 'resolved', decision: 'allow_session' })}
        resolving={false}
        onDecide={vi.fn()}
      />,
    );
    expect(screen.queryByRole('button', { name: 'Allow once' })).toBeNull();
    expect(screen.getByText('Allowed for session')).not.toBeNull();
  });

  it('shows an expired approval as such', () => {
    render(
      <ApprovalCard
        approval={view({ status: 'expired', decision: 'expired' })}
        resolving={false}
        onDecide={vi.fn()}
      />,
    );
    expect(screen.getByText('Expired')).not.toBeNull();
    expect(screen.queryByRole('button', { name: 'Deny' })).toBeNull();
  });

  it('renders diffs for file edits', () => {
    render(
      <ApprovalCard
        approval={view({
          kind: 'file_edit',
          title: 'Edit src/lib.rs',
          detail: 'Edit src/lib.rs',
          payload: {
            path: 'src/lib.rs',
            diff: '@@ -1 +1 @@\n-old\n+new\n',
          },
        })}
        resolving={false}
        onDecide={vi.fn()}
      />,
    );
    expect(screen.getAllByText('src/lib.rs').length).toBeGreaterThan(0);
    expect(screen.getByRole('group', { name: 'Unified diff' })).not.toBeNull();
    expect(screen.getByText('+new')).not.toBeNull();
  });

  it('gives the diff the mock file header with change counts', () => {
    render(
      <ApprovalCard
        approval={view({
          kind: 'file_edit',
          title: 'Edit src/lib.rs',
          payload: { path: 'src/lib.rs', diff: '@@ -1,2 +1,3 @@\n-old\n+new\n+extra\n ctx\n' },
        })}
        resolving={false}
        onDecide={vi.fn()}
      />,
    );
    const head = document.querySelector('.diff-head');
    expect(head).not.toBeNull();
    expect(head?.textContent).toContain('src/lib.rs');
    expect(screen.getByText('+2')).not.toBeNull();
    expect(screen.getByText('−1')).not.toBeNull();
  });

  it('shows the path and timeout as separate metadata rows', () => {
    render(
      <ApprovalCard
        approval={view({
          kind: 'file_write',
          title: 'Write build.rs',
          payload: { path: 'build.rs', timeout_secs: 300 },
        })}
        resolving={false}
        onDecide={vi.fn()}
      />,
    );
    expect(screen.getByText('path')).not.toBeNull();
    expect(screen.getByText('build.rs')).not.toBeNull();
    expect(screen.getByText('timeout')).not.toBeNull();
    expect(screen.getByText('300s')).not.toBeNull();
  });

  it('falls back to a JSON payload for kinds it cannot lay out', () => {
    render(
      <ApprovalCard
        approval={view({ kind: 'plugin', title: 'Run plugin tool', payload: { tool: 'fmt', scope: 'file' } })}
        resolving={false}
        onDecide={vi.fn()}
      />,
    );
    const payload = document.querySelector('.approval-payload');
    expect(payload?.textContent).toContain('"tool": "fmt"');
    expect(screen.getByText(/Plugins and MCP tools run with the server process permissions/)).not.toBeNull();
  });

  it('marks a denial with the negative tone', () => {
    render(
      <ApprovalCard approval={view({ status: 'resolved', decision: 'deny' })} resolving={false} onDecide={vi.fn()} />,
    );
    const result = screen.getByText('Denied');
    expect(result.className).toContain('is-negative');
  });

  it('names the card with its own heading for assistive tech', () => {
    render(<ApprovalCard approval={view()} resolving={false} onDecide={vi.fn()} />);
    const article = screen.getByRole('article');
    const titleId = article.getAttribute('aria-labelledby');
    expect(titleId).toBe('approval-title-ap_1');
    expect(document.getElementById(titleId ?? '')?.textContent).toBe('Run cargo test');
    expect(screen.getByText('Shell command')).not.toBeNull();
  });
});