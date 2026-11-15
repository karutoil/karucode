import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/preact';
import type { PluginDiagnostics, PluginToolOutputSummary } from '../api/types';
import { decidePluginToolOutput, clearPluginToolOutput } from '../api/endpoints';
import { PluginToolOutputRow, formatBytes, formatWhen } from './PluginToolOutput';

vi.mock('../api/endpoints', () => ({
  decidePluginToolOutput: vi.fn(() => Promise.resolve({})),
  clearPluginToolOutput: vi.fn(() => Promise.resolve({})),
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function plugin(budget: PluginToolOutputSummary): PluginDiagnostics {
  return {
    name: 'kc-codegraph',
    status: 'ready',
    tools: [],
    pid: 42,
    restart_count: 0,
    tool_output: budget,
  };
}

const GLOBAL: PluginToolOutputSummary = {
  effective_bytes: 24_000,
  granted: false,
  global_bytes: 24_000,
  ceiling_bytes: 1_000_000,
  pending: null,
};

/**
 * The budget row is where the operator answers the ask, so the tests are about
 * the two things that must be true of it: it shows the value the runtime really
 * applies, and it never presents an unanswered request as if it were one.
 */
describe('PluginToolOutputRow', () => {
  it('shows the global default when nothing was granted', () => {
    render(<PluginToolOutputRow plugin={plugin(GLOBAL)} onChanged={() => {}} />);
    expect(screen.getByText(/Output budget 24,000 B/)).toBeTruthy();
    expect(screen.getByText(/global default/)).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Allow' })).toBeNull();
  });

  it('shows a granted budget with who granted it and when', () => {
    render(
      <PluginToolOutputRow
        plugin={plugin({
          ...GLOBAL,
          effective_bytes: 400_000,
          granted: true,
          granted_bytes: 400_000,
          decided_by: 'operator',
          decided_at: Date.UTC(2026, 1, 3, 14, 5),
        })}
        onChanged={() => {}}
      />,
    );
    expect(screen.getByText(/Output budget 400,000 B/)).toBeTruthy();
    expect(screen.getByText(/granted by operator/)).toBeTruthy();
  });

  it('asks the question, and an Allow answers it with the requested value', async () => {
    const onChanged = vi.fn();
    render(
      <PluginToolOutputRow
        plugin={plugin({
          ...GLOBAL,
          pending: { bytes: 400_000, reason: 'whole-feature bundles', requested_at: 1 },
        })}
        onChanged={onChanged}
      />,
    );
    // The ask is visible, with its reason, and it is still the global default:
    // a request is not a grant.
    expect(screen.getByText(/asks for 400,000 bytes/)).toBeTruthy();
    expect(screen.getByText(/whole-feature bundles/)).toBeTruthy();
    expect(screen.getByText(/Output budget 24,000 B/)).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Allow' }));
    await waitFor(() => expect(decidePluginToolOutput).toHaveBeenCalledTimes(1));
    expect(vi.mocked(decidePluginToolOutput).mock.calls[0]!.slice(0, 3)).toEqual([
      'kc-codegraph',
      'grant',
      400_000,
    ]);
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it('a Refuse records a refusal rather than a budget', async () => {
    render(
      <PluginToolOutputRow
        plugin={plugin({
          ...GLOBAL,
          pending: { bytes: 400_000, reason: '', requested_at: 1 },
        })}
        onChanged={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Refuse' }));
    await waitFor(() => expect(decidePluginToolOutput).toHaveBeenCalledTimes(1));
    const call = vi.mocked(decidePluginToolOutput).mock.calls[0]!;
    expect(call[1]).toBe('refuse');
    expect(call[2]).toBeNull();
  });

  it('will not grant a value outside the accepted range', () => {
    render(
      <PluginToolOutputRow
        plugin={plugin({
          ...GLOBAL,
          pending: { bytes: 400_000, reason: '', requested_at: 1 },
        })}
        onChanged={() => {}}
      />,
    );
    const input = screen.getByLabelText('Bytes to allow') as HTMLInputElement;
    fireEvent.input(input, { target: { value: '5' } });
    fireEvent.click(screen.getByRole('button', { name: 'Allow' }));
    expect(decidePluginToolOutput).not.toHaveBeenCalled();
    expect(screen.getByRole('alert').textContent).toMatch(/between 2,000 and 1,000,000/);
  });

  it('Clear forgets an answer so it can be asked again', async () => {
    render(
      <PluginToolOutputRow
        plugin={plugin({ ...GLOBAL, effective_bytes: 400_000, granted: true, decided_by: 'operator' })}
        onChanged={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    await waitFor(() => expect(clearPluginToolOutput).toHaveBeenCalledWith('kc-codegraph', undefined));
  });
});

describe('budget formatting', () => {
  it('groups thousands and never prints a bare undefined', () => {
    expect(formatBytes(24_000)).toBe('24,000');
    expect(formatBytes(null)).toBe('—');
    expect(formatBytes(undefined)).toBe('—');
  });

  it('renders a timestamp, and nothing for an absent one', () => {
    expect(formatWhen(0)).toBe('—');
    expect(formatWhen(null)).toBe('—');
    expect(formatWhen(Date.UTC(2026, 1, 3, 14, 5))).toMatch(/3 Feb 2026/);
  });
});