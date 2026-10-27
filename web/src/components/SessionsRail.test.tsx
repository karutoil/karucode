import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { SessionList, type SessionListProps } from './SessionsRail';
import { secondSummaryFixture, summaryFixture, NOW } from '../test/fixtures';

function renderList(overrides: Partial<SessionListProps> = {}) {
  const props: SessionListProps = {
    sessions: [summaryFixture, secondSummaryFixture],
    currentId: 's_ab12',
    now: NOW,
    loading: false,
    workspace: '/home/me/project',
    onCreate: vi.fn(),
    onSelect: vi.fn(),
    onRename: vi.fn(),
    onDelete: vi.fn(),
    ...overrides,
  };
  const result = render(<SessionList {...props} />);
  return { ...result, props };
}

describe('SessionList', () => {
  it('renders titles, relative times, message counts, status dots and the active session', () => {
    const { container } = renderList();

    expect(screen.getByText('Fix login bug')).not.toBeNull();
    expect(screen.getByText('Add workspace tree')).not.toBeNull();
    expect(screen.getByText('2m ago')).not.toBeNull();
    expect(screen.getByText('12')).not.toBeNull();
    expect(screen.getByText('3')).not.toBeNull();

    const rows = container.querySelectorAll('.session-row');
    expect(rows).toHaveLength(2);
    expect(rows[0]?.querySelector('.session-open')?.getAttribute('aria-current')).toBe('true');
    expect(rows[1]?.querySelector('.session-open')?.getAttribute('aria-current')).toBeNull();
    expect(rows[1]?.querySelector('.status-dot.status-danger')).not.toBeNull();
  });

  it('shows a running dot with screen-reader text', () => {
    const { container } = renderList({
      sessions: [{ ...summaryFixture, running: true, last_run_status: 'running' }],
    });
    expect(container.querySelector('.status-dot.status-running')).not.toBeNull();
    expect(screen.getByText('Running.')).not.toBeNull();
  });

  it('selects a session when clicked', () => {
    const { props } = renderList();
    fireEvent.click(screen.getByText('Add workspace tree'));
    expect(props.onSelect).toHaveBeenCalledWith('s_cd34');
  });

  it('renames inline on double-click and commits on blur', () => {
    const { props } = renderList();
    fireEvent.dblClick(screen.getByText('Fix login bug'));
    const input = screen.getByLabelText(/Rename session/) as HTMLInputElement;
    expect(input.value).toBe('Fix login bug');
    fireEvent.input(input, { target: { value: 'Renamed session' } });
    fireEvent.blur(input);
    expect(props.onRename).toHaveBeenCalledWith('s_ab12', 'Renamed session');
  });

  it('cancels inline rename on Escape', () => {
    const { props } = renderList();
    fireEvent.dblClick(screen.getByText('Fix login bug'));
    const input = screen.getByLabelText(/Rename session/);
    fireEvent.input(input, { target: { value: 'Discard this' } });
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(props.onRename).not.toHaveBeenCalled();
  });

  it('asks for confirmation before deleting', () => {
    const { props } = renderList();
    fireEvent.click(screen.getByRole('button', { name: 'Actions for Fix login bug' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete…' }));

    const dialog = screen.getByRole('dialog');
    expect(dialog.textContent).toContain('Delete session?');
    expect(props.onDelete).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Delete session' }));
    expect(props.onDelete).toHaveBeenCalledWith('s_ab12');
  });

  it('shows an explanatory empty state with a working create button', () => {
    const { props } = renderList({ sessions: [], currentId: null });
    expect(screen.getByText('No sessions yet')).not.toBeNull();
    expect(screen.getByText(/A session keeps one conversation/)).not.toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /New session/ }));
    expect(props.onCreate).toHaveBeenCalledTimes(1);
  });

  it('shows loading skeletons before the first response', () => {
    const { container } = renderList({ sessions: [], loading: true });
    expect(container.querySelectorAll('.skeleton-session').length).toBeGreaterThan(0);
    expect(screen.getByLabelText('Loading sessions')).not.toBeNull();
  });
});