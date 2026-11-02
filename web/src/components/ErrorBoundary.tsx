import { Component } from 'preact';
import type { ComponentChildren, JSX } from 'preact';
import { copyText } from '../lib/clipboard';

export interface ErrorBoundaryProps {
  children: ComponentChildren;
  /** What broke, for the card's heading: "the workspace", "this conversation". */
  scope?: string;
  /**
   * Marks the outermost boundary, the one with nothing of the app left outside it.
   *
   * The difference matters because the card's advice does: a nested boundary took
   * one panel down and the rest of the workspace really is still running, while
   * the root boundary caught a crash and there is nothing on screen to go back to.
   * `App` sets this on its shell boundary; every panel boundary leaves it off.
   */
  root?: boolean;
}

interface ErrorBoundaryState {
  error: Error | null;
  copied: boolean;
}

function toError(value: unknown): Error {
  if (value instanceof Error) return value;
  return new Error(typeof value === 'string' ? value : String(value));
}

/** Everything a bug report needs, in one pasteable block. */
export function diagnostics(error: Error): string {
  return [
    `karucode web error: ${error.name}: ${error.message}`,
    error.stack ?? '(no stack)',
    `url: ${typeof location === 'undefined' ? 'n/a' : location.href}`,
    `agent: ${typeof navigator === 'undefined' ? 'n/a' : navigator.userAgent}`,
    `at: ${new Date().toISOString()}`,
  ].join('\n');
}

/**
 * The last line of defence for a render that throws.
 *
 * A malformed payload that slips past the reducer (an `approval_requested`
 * without its `approval`, say) would otherwise unmount the whole tree and leave
 * a white page with no way back but a manual reload. This catches it, keeps the
 * message on screen and offers the two things that actually help: reload, or
 * copy the diagnostics into a bug report.
 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  override state: ErrorBoundaryState = { error: null, copied: false };

  static override getDerivedStateFromError(error: unknown): Partial<ErrorBoundaryState> {
    return { error: toError(error) };
  }

  override componentDidCatch(error: unknown): void {
    // The card is the user-facing half; the console keeps the trail for reports.
    console.error('karucode: a component threw while rendering', error);
  }

  private reload = (): void => {
    try {
      window.location.reload();
    } catch {
      // jsdom and locked-down browsers refuse navigation; the card stays usable.
    }
  };

  private copy = (): void => {
    const error = this.state.error;
    if (!error) return;
    void copyText(diagnostics(error)).then((ok) => {
      if (ok) this.setState({ copied: true });
    });
  };

  render(): JSX.Element {
    const { error, copied } = this.state;
    if (!error) return <>{this.props.children}</>;

    const scope = this.props.scope ?? 'this view';
    // What the reader can still do depends on which boundary caught. A nested one
    // took a single panel down: everything else on the page is alive and usable,
    // and reloading would throw away the session state they are working in. The
    // root one has nothing left outside it, so reload is the only way forward.
    const hint = this.props.root
      ? 'Nothing else is available: this was the outermost boundary. Reload to get back to a clean state, or copy the diagnostics into a bug report.'
      : 'The rest of the workspace is still running. Reload to get back to a clean state, or copy the diagnostics into a bug report.';
    return (
      <div class="crash-card" role="alert">
        <p class="crash-kicker">Something broke</p>
        <h2 class="crash-title">{scope} could not be rendered.</h2>
        <p class="crash-message">{error.message || error.name}</p>
        <p class="crash-hint">{hint}</p>
        <div class="crash-actions">
          <button type="button" class="button button-primary button-small" onClick={this.reload}>
            Reload
          </button>
          <button type="button" class="button button-small" onClick={this.copy}>
            {copied ? 'Copied' : 'Copy diagnostics'}
          </button>
        </div>
      </div>
    );
  }
}
