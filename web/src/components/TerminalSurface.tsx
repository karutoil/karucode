import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { FitAddon } from '@xterm/addon-fit';
import { Terminal as XTerm } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import type { Terminal } from '../api/types';
import { toErrorMessage } from '../api/client';
import * as terminalsState from '../state/terminals';
import { TerminalSocket, type TerminalSocketStatus } from '../state/terminalSocket';
import { RefreshIcon } from './icons';

/**
 * One terminal's xterm instance and its socket.
 *
 * Keyed by terminal id by the parent, so switching tabs mounts a fresh surface
 * for the newly selected terminal. The `instances` map keeps the *xterm* object
 * alive across that remount so scrollback and the cursor survive the switch;
 * the socket is per-surface and detaches on unmount.
 */
const instances = new Map<string, { term: XTerm; fit: FitAddon }>();

function getInstance(id: string): { term: XTerm; fit: FitAddon } {
  const existing = instances.get(id);
  if (existing) return existing;
  const fit = new FitAddon();
  const term = new XTerm({
    // The server's replay is the source of truth for content, so the local
    // buffer only needs to be long enough to scroll a screenful or two of the
    // replayed tail.
    scrollback: 5000,
    cursorBlink: true,
    convertEol: false,
    fontFamily:
      'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace',
    fontSize: 13,
    lineHeight: 1.25,
    theme: terminalTheme(),
    allowProposedApi: true,
  });
  term.loadAddon(fit);
  instances.set(id, { term, fit });
  return { term, fit };
}

/**
 * Colours for xterm, read from the live theme tokens.
 *
 * xterm draws to a canvas and cannot use CSS custom properties, so the values
 * are resolved once per surface. A theme change while the tab is open repaints
 * on the next mount; a live theme switch does not need to retint a shell.
 */
function terminalTheme(): Record<string, string> {
  const read = (name: string, fallback: string): string => {
    if (typeof window === 'undefined' || typeof window.getComputedStyle !== 'function') {
      return fallback;
    }
    const value = window
      .getComputedStyle(document.documentElement)
      .getPropertyValue(name)
      .trim();
    return value || fallback;
  };
  const background = read('--surface-sunken', read('--bg', '#16181d'));
  const foreground = read('--text', '#d7dbe3');
  return {
    background,
    foreground,
    cursor: read('--accent', '#7aa2f7'),
    cursorAccent: background,
    selectionBackground: read('--surface-hover', 'rgba(122,162,247,0.25)'),
    black: read('--ink-secondary', '#414868'),
    red: read('--danger', '#f7768e'),
    green: read('--success', '#9ece6a'),
    yellow: read('--warning', '#e0af68'),
    blue: read('--accent', '#7aa2f7'),
    magenta: '#bb9af7',
    cyan: '#7dcfff',
    white: foreground,
  };
}

export function TerminalSurface({ terminal }: { terminal: Terminal }): JSX.Element {
  const hostRef = useRef<HTMLDivElement>(null);
  const socketRef = useRef<TerminalSocket | null>(null);
  const [status, setStatus] = useState<TerminalSocketStatus>('connecting');
  const [detail, setDetail] = useState<string | null>(null);
  const [exited, setExited] = useState<number | null>(null);
  const [reconnectTick, setReconnectTick] = useState(0);

  const { term, fit } = useMemo(() => getInstance(terminal.id), [terminal.id]);

  // Mount the xterm instance into this surface's host. `open()` is idempotent
  // per element, and re-opening into a new element is how the instance moves
  // between tabs without losing its buffer.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    term.open(host);
    const measure = (): void => {
      try {
        fit.fit();
      } catch {
        // A hidden element measures as zero; the observer fires again when it
        // is shown.
      }
    };
    measure();
    // The size the shell needs is the *rendered* size, which is only known
    // after layout, so fit on the next frame as well.
    const frame = window.requestAnimationFrame(measure);
    const observer =
      typeof ResizeObserver === 'function' ? new ResizeObserver(measure) : null;
    observer?.observe(host);
    return () => {
      window.cancelAnimationFrame(frame);
      observer?.disconnect();
    };
  }, [term, fit]);

  // Keep the tab's status in the shared store so the strip can draw a dot.
  useEffect(() => {
    terminalsState.setTerminalStatus(terminal.id, status);
  }, [terminal.id, status]);

  // Attach. Re-runs on `reconnectTick` so the button re-opens the socket.
  useEffect(() => {
    setExited(null);
    setDetail(null);
    const cols = term.cols || 80;
    const rows = term.rows || 24;
    const socket = new TerminalSocket({
      id: terminal.id,
      cols,
      rows,
      handlers: {
        onData: (bytes) => term.write(bytes),
        onControl: (frame) => {
          if (frame.type === 'ready') {
            // The server replays the scrollback as the next binary frame. Clear
            // first so a reconnect does not append onto a stale screen.
            if (frame.running) term.clear();
            setExited(null);
          } else if (frame.type === 'exit') {
            setExited(frame.code ?? -1);
          } else if (frame.type === 'lagged') {
            setDetail('Output was dropped while the page was busy; reconnecting to resync.');
          } else if (frame.type === 'detached') {
            setDetail('Another browser attached to this terminal.');
          } else if (frame.type === 'error') {
            setDetail(frame.message);
          }
        },
        onStatus: (next, why) => {
          setStatus(next);
          if (why) setDetail(why);
        },
      },
    });
    socketRef.current = socket;
    socket.connect();

    // Keystrokes go to the shell as raw bytes, not as a JSON string: the browser
    // sends exactly what the PTY expects.
    const dataListener = term.onData((data) => socket.send(new TextEncoder().encode(data)));
    // A binary reply (a program writing to stdout in response to a query) is
    // already handled by the data path above.

    return () => {
      dataListener.dispose();
      socket.dispose();
      socketRef.current = null;
    };
  }, [terminal.id, term, reconnectTick]);

  // Resize: xterm's own resize event (from the FitAddon) is the trigger, so a
  // window resize or a rail collapse both reach the shell.
  useEffect(() => {
    const resizeListener = term.onResize(({ cols, rows }) => {
      socketRef.current?.resize(cols, rows);
    });
    return () => resizeListener.dispose();
  }, [term]);

  const reconnect = (): void => {
    term.clear();
    setReconnectTick((tick) => tick + 1);
  };

  const restart = async (): Promise<void> => {
    term.clear();
    await terminalsState.restartTerminalShell(terminal.id);
    setReconnectTick((tick) => tick + 1);
  };

  return (
    <div
      class={`terminal-surface${exited !== null ? ' is-exited' : ''}`}
      id={`terminal-panel-${terminal.id}`}
      role="tabpanel"
      aria-labelledby={`terminal-tab-${terminal.id}`}
      tabIndex={-1}
    >
      <div class="terminal-meta">
        <span class={`terminal-state status-${status}`} title={detail ?? status}>
          {statusLabel(status)}
        </span>
        <span class="terminal-cwd" title={terminal.cwd}>
          {terminal.cwd}
        </span>
        <div class="terminal-meta-actions">
          <button
            type="button"
            class="terminal-action"
            onClick={() => void restart()}
            title="Kill the shell and start a new one in the same directory"
          >
            <RefreshIcon size={12} /> Restart
          </button>
        </div>
      </div>

      {exited !== null ? (
        <div class="terminal-exit" role="status">
          <span>
            The shell exited{exited >= 0 ? ` with code ${exited}` : ''}. The tab stays; reopen it
            to start a new shell in the same directory.
          </span>
          <button type="button" class="button button-small" onClick={reconnect}>
            <RefreshIcon size={13} /> Reconnect
          </button>
        </div>
      ) : status === 'failed' || status === 'closed' ? (
        <div class="terminal-exit" role="status">
          <span>{detail ?? toErrorMessage('The connection to the shell closed.')}</span>
          <button type="button" class="button button-small" onClick={reconnect}>
            <RefreshIcon size={13} /> Reconnect
          </button>
        </div>
      ) : null}

      {/* The xterm canvas lives here. Focus lands on xterm's own textarea, so a
          click anywhere in the body types into the shell. */}
      <div class="terminal-host" ref={hostRef} />
    </div>
  );
}

function statusLabel(status: TerminalSocketStatus): string {
  switch (status) {
    case 'open':
      return 'connected';
    case 'connecting':
      return 'connecting';
    case 'reconnecting':
      return 'reconnecting';
    case 'failed':
      return 'failed';
    case 'closed':
      return 'closed';
    default:
      return 'idle';
  }
}
