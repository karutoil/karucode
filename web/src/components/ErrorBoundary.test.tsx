import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/preact';
import type { JSX } from 'preact';
import { ErrorBoundary, diagnostics } from './ErrorBoundary';

/** A child that throws while rendering, the way a malformed payload would. */
function Exploding({ message = 'event.approval is undefined' }: { message?: string }): JSX.Element {
  throw new Error(message);
}

function renderInBoundary(children: JSX.Element, scope?: string) {
  return render(<ErrorBoundary scope={scope}>{children}</ErrorBoundary>);
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('ErrorBoundary', () => {
  it('renders its children when nothing throws', () => {
    renderInBoundary(<p>the transcript</p>);
    expect(screen.getByText('the transcript')).not.toBeNull();
    expect(screen.queryByText('Something broke')).toBeNull();
  });

  it('renders the fallback card instead of blanking the page', () => {
    // The boundary logs the error on purpose; keep the test output clean.
    vi.spyOn(console, 'error').mockImplementation(() => undefined);

    renderInBoundary(<Exploding />, 'This conversation');

    // The card, not an empty document: this is the whole point of the fix.
    expect(screen.getByRole('alert')).not.toBeNull();
    expect(screen.getByText('Something broke')).not.toBeNull();
    expect(screen.getByText('This conversation could not be rendered.')).not.toBeNull();
    // The message is on screen, so a bug report can quote it.
    expect(screen.getByText('event.approval is undefined')).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Reload' })).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Copy diagnostics' })).not.toBeNull();
  });

  it('keeps the rest of the tree alive when only one child throws', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);

    render(
      <div>
        <p>the rails are fine</p>
        <ErrorBoundary scope="This conversation">
          <Exploding />
        </ErrorBoundary>
      </div>,
    );

    expect(screen.getByText('the rails are fine')).not.toBeNull();
    expect(screen.getByRole('alert')).not.toBeNull();
  });

  it('promises the rest of the workspace is running only when that is true', () => {
    // A nested boundary took one panel down: the rail, the transcript and the
    // composer are still mounted and usable, so the copy is accurate and reloading
    // would throw away a live session for nothing.
    vi.spyOn(console, 'error').mockImplementation(() => undefined);

    const nested = render(
      <ErrorBoundary scope="The inspector">
        <Exploding />
      </ErrorBoundary>,
    );
    expect(screen.getByText(/The rest of the workspace is still running/)).not.toBeNull();
    nested.unmount();

    // The root boundary has nothing left outside it. Saying "the rest of the
    // workspace is still running" there was a lie: the page is one crash card.
    render(
      <ErrorBoundary scope="The workspace" root>
        <Exploding />
      </ErrorBoundary>,
    );
    expect(screen.queryByText(/The rest of the workspace is still running/)).toBeNull();
    expect(screen.getByText(/this was the outermost boundary/)).not.toBeNull();
  });

  it('copies a diagnostics block that names the error and the page', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const writeText = vi.fn(async (_text: string) => undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });

    renderInBoundary(<Exploding message="boom" />);
    screen.getByRole('button', { name: 'Copy diagnostics' }).click();

    await vi.waitFor(() => expect(writeText).toHaveBeenCalled());
    const copied = writeText.mock.calls[0]?.[0] as unknown as string;
    expect(copied).toContain('karucode web error: Error: boom');
    expect(copied).toContain('url:');
    expect(copied).toContain('agent:');
  });

  it('formats non-Error throws without losing the message', () => {
    expect(diagnostics(new Error('plain'))).toContain('Error: plain');
  });
});
