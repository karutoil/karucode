import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { FitAddon } from '@xterm/addon-fit';
import { Terminal as XTerm } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import type { Terminal } from '../api/types';
import { toErrorMessage } from '../api/client';
import { activeTheme, contrastRatio, mix, normalizeHex, rgbToHex } from '../theme';
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
 *
 * The emulator is disposed by `disposeTerminal` when the tab is closed. Nothing
 * else releases it: the map is module state, so an entry that is never disposed
 * keeps its emulator, FitAddon, canvas and listeners alive for the life of the
 * page - up to `terminals.max_terminals` (16) of them.
 */
const instances = new Map<
  string,
  { term: XTerm; fit: FitAddon; appearance: TerminalAppearance }
>();

/**
 * Release a terminal's emulator. Called when the terminal is closed, so closing
 * a tab (or letting the server prune one) frees the canvas and the listeners
 * instead of leaking them until reload.
 *
 * Safe to call for an id that was never opened.
 */
export function disposeTerminal(id: string): void {
  const instance = instances.get(id);
  if (!instance) return;
  instances.delete(id);
  try {
    instance.fit.dispose();
  } catch {
    // A partially-constructed addon has nothing to release.
  }
  try {
    instance.term.dispose();
  } catch {
    // `dispose()` throws if xterm was never `open()`ed in some versions; the
    // map entry is already gone, which is what matters for the leak.
  }
}

/** Test seam: how many emulators are currently held. */
export function terminalInstanceCount(): number {
  return instances.size;
}

function getInstance(id: string): {
  term: XTerm;
  fit: FitAddon;
  appearance: TerminalAppearance;
} {
  const existing = instances.get(id);
  if (existing) return existing;
  const fit = new FitAddon();
  const appearance = terminalAppearance();
  const term = new XTerm({
    // The server's replay is the source of truth for content, so the local
    // buffer only needs to be long enough to scroll a screenful or two of the
    // replayed tail.
    scrollback: 5000,
    cursorBlink: true,
    convertEol: false,
    // Read from the type scale so a density change moves the shell too.
    fontFamily: appearance.fontFamily,
    fontSize: appearance.fontSize,
    lineHeight: 1.25,
    theme: appearance.theme,
    allowProposedApi: true,
  });
  term.loadAddon(fit);
  const instance = { term, fit, appearance };
  instances.set(id, instance);
  return instance;
}

/** The cell size, from the type scale rather than a magic number. */
function terminalFontSize(): number {
  if (typeof window === 'undefined' || typeof window.getComputedStyle !== 'function') return 13;
  const value = window.getComputedStyle(document.documentElement).getPropertyValue('--fs-body').trim();
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 13;
}

const DEFAULT_FONT_FAMILY =
  'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace';

/** One live token, with the fallback the first paint would have used. */
function readToken(name: string, fallback: string): string {
  if (typeof window === 'undefined' || typeof window.getComputedStyle !== 'function') {
    return fallback;
  }
  const value = window.getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value || fallback;
}

/** The three colour channels of an rgb()/rgba() functional colour, before alpha. */
function rgbFunctionTokens(value: string): string[] | null {
  const match = /^rgba?\((.*)\)$/i.exec(value.trim());
  if (!match) return null;
  const body = (match[1] ?? '').split('/')[0]?.trim() ?? '';
  const tokens = body.includes(',') ? body.split(',').map((part) => part.trim()) : body.split(/\s+/);
  return tokens.length >= 3 ? tokens.slice(0, 3) : null;
}

function hslFunctionHex(value: string): string | null {
  const match = /^hsla?\((.*)\)$/i.exec(value.trim());
  if (!match) return null;
  const body = (match[1] ?? '').split('/')[0]?.trim() ?? '';
  const tokens = body.includes(',') ? body.split(',').map((part) => part.trim()) : body.split(/\s+/);
  if (tokens.length < 3) return null;
  const hue = Number.parseFloat(tokens[0] ?? '');
  const saturation = Number.parseFloat(tokens[1] ?? '') / 100;
  const lightness = Number.parseFloat(tokens[2] ?? '') / 100;
  if (
    !Number.isFinite(hue) ||
    !/^[-+]?(?:\d+\.?\d*|\.\d+)(?:deg)?$/i.test(tokens[0] ?? '') ||
    !tokens[1]?.endsWith('%') ||
    !tokens[2]?.endsWith('%') ||
    !Number.isFinite(saturation) ||
    !Number.isFinite(lightness)
  ) return null;
  const h = ((hue % 360) + 360) % 360;
  const s = Math.min(1, Math.max(0, saturation));
  const l = Math.min(1, Math.max(0, lightness));
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const p = 2 * l - q;
  const channel = (offset: number): number => {
    let t = h / 360 + offset;
    if (t < 0) t += 1;
    if (t > 1) t -= 1;
    if (t < 1 / 6) return p + (q - p) * 6 * t;
    if (t < 1 / 2) return q;
    if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
    return p;
  };
  return rgbToHex([channel(1 / 3) * 255, channel(0) * 255, channel(-1 / 3) * 255]);
}

/** Convert browser-valid CSS colours to the hex-only input xterm's blends need. */
function terminalHex(value: string): string | null {
  const direct = normalizeHex(value);
  if (direct) return direct;
  const rgbTokens = rgbFunctionTokens(value);
  if (rgbTokens) {
    const channels = rgbTokens.map((token) => {
      const percentage = token.endsWith('%');
      const numeric = token.slice(0, percentage ? -1 : undefined);
      if (!/^[+-]?(?:\d+\.?\d*|\.\d+)$/.test(numeric)) return null;
      const channel = Number(numeric);
      return Number.isFinite(channel) ? channel * (percentage ? 2.55 : 1) : null;
    });
    const [red, green, blue] = channels;
    if (typeof red === 'number' && typeof green === 'number' && typeof blue === 'number') {
      return rgbToHex([red, green, blue]);
    }
  }
  const hsl = hslFunctionHex(value);
  if (hsl) return hsl;

  // Named, system or newer CSS colour syntax is left to the browser's own
  // canonical RGB answer. Detached/jsdom fallbacks still get a safe conversion.
  if (typeof document === 'undefined' || typeof window?.getComputedStyle !== 'function') return null;
  const probe = document.createElement('span');
  probe.style.color = value;
  if (probe.style.color === '') return null;
  probe.style.position = 'absolute';
  probe.style.visibility = 'hidden';
  probe.style.pointerEvents = 'none';
  document.documentElement.appendChild(probe);
  try {
    const canonical = window.getComputedStyle(probe).color || probe.style.color;
    if (canonical !== value) return terminalHex(canonical);
    return null;
  } finally {
    probe.remove();
  }
}

function mixTerminal(from: string, to: string, amount: number): string {
  const a = terminalHex(from);
  const b = terminalHex(to);
  if (a && b) return mix(a, b, amount);
  // An unknown colour must not take down the whole workspace. Prefer the known
  // endpoint; the directly supplied terminal colours still remain on screen.
  return a ?? b ?? '#000000';
}

/**
 * The mono stack from the Theme studio, so choosing a code font retypes the
 * shell with the rest of the app instead of leaving it on the default.
 */
function terminalFontFamily(): string {
  return readToken('--font-mono', DEFAULT_FONT_FAMILY);
}

/** Everything xterm has to be told again when the theme changes. */
interface TerminalAppearance {
  fontSize: number;
  fontFamily: string;
  theme: Record<string, string>;
}

function terminalAppearance(): TerminalAppearance {
  return {
    fontSize: terminalFontSize(),
    fontFamily: terminalFontFamily(),
    theme: terminalTheme(),
  };
}

function sameTheme(a: Record<string, string>, b: Record<string, string>): boolean {
  const keys = Object.keys(a);
  return (
    keys.length === Object.keys(b).length &&
    keys.every((key) => Object.hasOwn(b, key) && a[key] === b[key])
  );
}

/**
 * Colours for xterm, read from the live theme tokens.
 *
 * xterm draws to a canvas and cannot use CSS custom properties, so the values
 * are resolved whenever the theme changes and pushed back into the cached
 * emulator's options (see the appearance effect in `TerminalSurface`).
 */
function terminalTheme(): Record<string, string> {
  const read = (name: string, fallback: string): string => readToken(name, fallback);
  // The canvas xterm paints on is `.terminal-surface`'s `--bg-sunken`. Reading a
  // different token (`--surface-sunken`, which is a *line* tone) left the emulator
  // believing its background was one colour while the CSS painted another, so
  // selection, erase and the contrast maths inside xterm all used the wrong one.
  const background = read('--bg-sunken', read('--bg', '#16181d'));
  const foreground = read('--text', '#d7dbe3');
  // The ANSI ramp is *derived from this theme*, never written by hand. It used to
  // map `blue` to `--accent`, so in Ember an amber accent painted `ls` in amber —
  // colour 4 was not blue. `magenta` and `cyan` were the Tokyo Night hexes from
  // an unrelated palette, and `black` read `--ink-secondary`, which does not
  // exist, so every program that used colour 0 was invisible.
  const danger = read('--danger', '#f7768e');
  const success = read('--success', '#9ece6a');
  const warn = read('--warn', '#e0af68');
  const info = read('--info', '#7aa2f7');
  const muted = read('--muted', '#8b92a1');
  const lighten = (colour: string, amount = 0.28): string => mixTerminal(colour, foreground, amount);
  // Colour 0 is a *foreground*, so "readable" is a property of the theme rather
  // than a constant blend. 22% of the ink over Ember's near-black canvas is a mid
  // grey at 4.4:1 (fine); the same 22% over Daybreak's near-white canvas is a pale
  // grey at 1.74:1 (invisible) - which is the same defect the fixed blend was
  // introduced to fix, just in the other mode. Walk the blend up until it clears
  // the §4 4.5:1 floor, so one formula is right in every theme.
  const readable = (ink: string, floor = 4.5): string => {
    const base = terminalHex(background);
    const color = terminalHex(ink);
    if (!base || !color) return mixTerminal(background, ink, 0.22);
    for (let amount = 0.2; amount < 0.95; amount += 0.05) {
      const candidate = mix(base, color, amount);
      if (contrastRatio(candidate, base) >= floor) return candidate;
    }
    return mix(base, color, 0.95);
  };
  return {
    background,
    foreground,
    cursor: read('--accent', '#7aa2f7'),
    cursorAccent: background,
    selectionBackground: read('--surface-hover', 'rgba(122,162,247,0.25)'),
    // Colour 0 is a foreground, so it must clear the background: a true black
    // measured 2.08:1 on a dark canvas, and a fixed blend measured 1.74:1 on a
    // light one. `readable()` is the only version that is right in both.
    black: readable(foreground),
    red: danger,
    green: success,
    yellow: warn,
    // Colour 4 is blue. `--info` is the palette's blue; the accent is not.
    blue: info,
    magenta: mixTerminal(danger, info, 0.5),
    cyan: mixTerminal(info, success, 0.5),
    white: foreground,
    brightBlack: muted,
    brightRed: lighten(danger),
    brightGreen: lighten(success),
    brightYellow: lighten(warn),
    brightBlue: lighten(info),
    brightMagenta: lighten(mixTerminal(danger, info, 0.5)),
    brightCyan: lighten(mixTerminal(info, success, 0.5)),
    brightWhite: lighten(foreground, 0.5),
  };
}

export function TerminalSurface({
  terminal,
  autoFocus = true,
}: {
  terminal: Terminal;
  autoFocus?: boolean;
}): JSX.Element {
  const hostRef = useRef<HTMLDivElement>(null);
  const socketRef = useRef<TerminalSocket | null>(null);
  const [status, setStatus] = useState<TerminalSocketStatus>('connecting');
  const [detail, setDetail] = useState<string | null>(null);
  const [exited, setExited] = useState<number | null>(null);
  const [reconnectTick, setReconnectTick] = useState(0);
  // Subscribing to the resolved theme is what makes a Theme studio edit reach a
  // shell that is already running; the signal changes before `applyTheme()` has
  // written the new custom properties, so the effect below reads them after.
  const theme = activeTheme.value;

  const instance = useMemo(() => getInstance(terminal.id), [terminal.id]);
  const { term, fit } = instance;

  // Let the tab strip release this emulator when the terminal is closed. The
  // disposer is registered from inside the lazily-loaded chunk so `state/terminals`
  // never has to import xterm.
  useEffect(() => {
    terminalsState.setTerminalEmulatorDisposer(disposeTerminal);
    return () => terminalsState.setTerminalEmulatorDisposer(null);
  }, []);

  // Mount the xterm instance into this surface's host.
  //
  // A tab switch remounts this component (the parent keys it by terminal id),
  // so this effect runs again with a *fresh* host element for an emulator that
  // may already have been opened. `Terminal.open()` is NOT a move: in xterm
  // 5.5.0 it early-returns as soon as `this.element` exists, so a second
  // `open(newHost)` leaves the element in the discarded host and the new pane
  // renders blank and untypable. Re-parent the element instead - that is what
  // actually moves the emulator - and only call `open()` on the first mount.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const element = term.element;
    if (element) {
      if (element.parentElement !== host) host.appendChild(element);
    } else {
      term.open(host);
    }
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
    // Pointer selection and a programmatically opened surface still move into
    // the shell. Keyboard activation is different: its focused tab owns the next
    // Arrow key, so stealing focus during this keyed remount would make the
    // second key go to the PTY instead of the tablist.
    if (autoFocus) {
      try {
        term.focus();
      } catch {
        // A hidden host cannot take focus; the observer retries the fit.
      }
    }
    return () => {
      window.cancelAnimationFrame(frame);
      observer?.disconnect();
    };
  }, [term, fit, autoFocus]);

  // Live theme, mode, mono stack and density.
  //
  // activeTheme also changes for rounding, transcript preferences, custom CSS
  // and other settings which may not alter a single terminal token. Resolve the
  // actual values first and retain them on the cached instance: only a real
  // colour difference paints again, and only a real font metric difference
  // writes font options and re-fits the grid (which still reaches onResize).
  // The buffer, scrollback, cursor and websocket are untouched.
  useEffect(() => {
    const appearance = terminalAppearance();
    const previous = instance.appearance;
    const fontChanged =
      appearance.fontSize !== previous.fontSize ||
      appearance.fontFamily !== previous.fontFamily;
    const colorsChanged = !sameTheme(appearance.theme, previous.theme);
    if (!fontChanged && !colorsChanged) return;
    instance.appearance = appearance;

    if (colorsChanged) term.options.theme = appearance.theme;
    if (fontChanged) {
      term.options.fontSize = appearance.fontSize;
      term.options.fontFamily = appearance.fontFamily;
      try {
        fit.fit();
      } catch {
        // A hidden element measures as zero; the observer retries the fit.
      }
    }
    if (colorsChanged) {
      try {
        term.refresh(0, Math.max(0, term.rows - 1));
      } catch {
        // A disposed emulator has nothing to repaint.
      }
    }
  }, [instance, term, fit, theme]);

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
