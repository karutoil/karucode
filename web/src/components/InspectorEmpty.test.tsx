import { cleanup, fireEvent, render, screen } from '@testing-library/preact';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { InspectorEmpty } from './InspectorEmpty';

afterEach(cleanup);

describe('InspectorEmpty', () => {
  it('draws a title, the explanation and one action', () => {
    const onClick = vi.fn();
    render(
      <InspectorEmpty title="No plan yet" action={{ label: 'Try again', onClick }}>
        When the agent plans multi-step work it records a task list here.
      </InspectorEmpty>,
    );
    expect(screen.getByText('No plan yet')).toBeTruthy();
    expect(screen.getByText(/records a task list here/)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('draws no control when the state has no action out of it', () => {
    render(<InspectorEmpty title="No session open">Open a session to read its numbers.</InspectorEmpty>);
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('keeps the shared empty-panel classes the inspector styles', () => {
    const { container } = render(<InspectorEmpty title="No plan yet">Nothing yet.</InspectorEmpty>);
    expect(container.querySelector('.empty-inspector')).not.toBeNull();
    expect(container.querySelector('.empty-inspector-title')?.textContent).toBe('No plan yet');
  });
});
