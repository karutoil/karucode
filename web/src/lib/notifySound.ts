/**
 * Notification cues, synthesized with Web Audio.
 *
 * ## Why there are no audio files
 *
 * The page CSP is `default-src 'self'; style-src 'self' 'unsafe-inline';
 * img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'none'`. There is
 * no `media-src`, so it falls back to `default-src 'self'`, and a `blob:` or
 * `data:` URL assigned to an `Audio` element is refused with `NotSupportedError`
 * (measured against the running server, not assumed). Shipping `.wav`/`.mp3` cue
 * files would therefore mean either adding `media-src` to the CSP for a handful of
 * beeps, or serving untrusted URLs — and neither is worth it when a cue is three
 * oscillators and an envelope.
 *
 * ## Why this is decoupled from notifications
 *
 * Sound and the browser Notification API are *unrelated*: `Notification.permission`
 * governs only the OS notification surface, and the AudioContext plays regardless.
 * That independence is the whole point of this module — a user who denies
 * notification permission still gets an audible cue — so nothing here reads
 * `Notification` at all.
 *
 * ## Autoplay
 *
 * Browsers block audio that starts outside a user gesture in a tab nobody has
 * interacted with. A workspace is always interacted with (you type a prompt), but
 * a page loaded in the background is not, so `arm()` is attached to the first
 * pointer/key event and the context is resumed there. If a cue is due while the
 * context is still suspended, `play()` resumes it and reports the failure instead
 * of throwing: a silent failure that looks like a bug is the outcome to avoid.
 */

/** The four cues. A kind maps to one of these. */
export type CueName = 'finished' | 'attention' | 'error' | 'tick';

/** How a cue is built: a small score, not a file. */
interface Note {
  /** Start offset in seconds from the beginning of the cue. */
  at: number;
  /** Frequency in Hz. */
  hz: number;
  /** Duration in seconds. */
  duration: number;
  /** Oscillator shape. */
  type: OscillatorType;
  /** Peak gain, 0..1. Kept well below 1: a notification must not startle. */
  peak: number;
}

/**
 * The cue table.
 *
 * Each cue is deliberately distinguishable by *shape*, not only pitch, so it is
 * recognisable on a laptop speaker: finished rises, attention repeats, error is
 * low and dull, tick is a single short blip.
 */
const CUES: Record<CueName, Note[]> = {
  // Two rising notes: "that's done".
  finished: [
    { at: 0, hz: 660, duration: 0.09, type: 'sine', peak: 0.18 },
    { at: 0.1, hz: 880, duration: 0.12, type: 'sine', peak: 0.16 },
  ],
  // Double ping, the highest-priority cue: something wants a human.
  attention: [
    { at: 0, hz: 880, duration: 0.1, type: 'sine', peak: 0.22 },
    { at: 0.14, hz: 880, duration: 0.1, type: 'sine', peak: 0.22 },
    { at: 0.28, hz: 990, duration: 0.14, type: 'sine', peak: 0.18 },
  ],
  // Low triangle with a slight beat, which reads as "wrong" rather than "loud".
  error: [
    { at: 0, hz: 220, duration: 0.26, type: 'triangle', peak: 0.2 },
    { at: 0.02, hz: 208, duration: 0.26, type: 'triangle', peak: 0.16 },
  ],
  // One short blip: informational, easy to ignore.
  tick: [{ at: 0, hz: 1200, duration: 0.045, type: 'sine', peak: 0.12 }],
};

/** Every cue name, in the order the settings screen lists them. */
export const CUE_NAMES: CueName[] = ['finished', 'attention', 'error', 'tick'];

/** Human labels for the settings screen and the preview button. */
export const CUE_LABELS: Record<CueName, string> = {
  finished: 'Run finished',
  attention: 'Needs attention',
  error: 'Error',
  tick: 'Info',
};

/** Longest a cue may be, used to reject a nonsense `sound` name cheaply. */
export const MAX_CUE_SECONDS = 1;

/**
 * The subset of `AudioContext` this module uses.
 *
 * Structural rather than the DOM type so a test can inject a fake without jsdom
 * having to provide Web Audio (it does not: `typeof AudioContext === 'undefined'`
 * there).
 */
export interface AudioContextLike {
  readonly state: string;
  readonly currentTime: number;
  readonly destination: unknown;
  resume(): Promise<void>;
  close(): Promise<void>;
  createOscillator(): OscillatorNodeLike;
  createGain(): GainNodeLike;
}

export interface OscillatorNodeLike {
  type: OscillatorType;
  frequency: { value: number; setValueAtTime?: (value: number, at: number) => void };
  connect(node: unknown): unknown;
  start(when?: number): void;
  stop(when?: number): void;
}

export interface GainNodeLike {
  gain: {
    value: number;
    setValueAtTime?: (value: number, at: number) => void;
    linearRampToValueAtTime?: (value: number, at: number) => void;
    exponentialRampToValueAtTime?: (value: number, at: number) => void;
  };
  connect(node: unknown): unknown;
}

/** Everything the engine needs from the outside world. */
export interface SoundEnvironment {
  /**
   * Build an AudioContext, or `null` when the browser has none. Called lazily and
   * at most once, so a browser without Web Audio costs nothing.
   */
  createContext: () => AudioContextLike | null;
  /** Timers, so a test can drive them. */
  setTimeout: (handler: () => void, ms: number) => number;
  clearTimeout: (handle: number) => void;
}

/** Why a cue did not play, for the caller to explain. */
export type PlayFailure = 'muted' | 'unsupported' | 'blocked' | 'coalesced';

export interface PlayResult {
  played: boolean;
  reason?: PlayFailure;
}

/**
 * The engine. One instance for the page: a second AudioContext would double every
 * cue and is a wasted resource.
 */
export class SoundEngine {
  private readonly env: SoundEnvironment;
  private context: AudioContextLike | null = null;
  private contextUnavailable = false;
  /** Set once a cue failed because the context could not be resumed. */
  private blocked = false;
  /** The last cue that started, so a burst plays once. */
  private lastCue: { name: CueName; at: number } | null = null;
  /** Handle for the "one cue per burst" window. */
  private coalesceTimer: number | null = null;
  private volume = 0.8;
  private muted: boolean;

  constructor(env: SoundEnvironment, options: { muted?: boolean; volume?: number } = {}) {
    this.env = env;
    this.muted = options.muted ?? false;
    this.volume = clampVolume(options.volume ?? 0.8);
  }

  /** Whether the engine currently believes sound is available. */
  status(): 'ready' | 'unsupported' | 'blocked' {
    if (this.contextUnavailable) return 'unsupported';
    if (this.blocked) return 'blocked';
    return 'ready';
  }

  isMuted(): boolean {
    return this.muted;
  }

  setMuted(muted: boolean): void {
    this.muted = muted;
  }

  getVolume(): number {
    return this.volume;
  }

  setVolume(volume: number): void {
    this.volume = clampVolume(volume);
  }

  /**
   * Create (or resume) the context from inside a user gesture.
   *
   * Call this from a `pointerdown`/`keydown` listener. It is idempotent and cheap
   * once the context is running, so a listener may call it on every gesture.
   */
  async arm(): Promise<boolean> {
    const context = this.ensureContext();
    if (!context) return false;
    if (context.state === 'running') {
      this.blocked = false;
      return true;
    }
    try {
      await context.resume();
      this.blocked = context.state !== 'running';
      return !this.blocked;
    } catch {
      // A resume outside a gesture can reject. Not an error worth surfacing: the
      // next gesture tries again.
      this.blocked = true;
      return false;
    }
  }

  /**
   * Play one cue.
   *
   * Never throws and never returns a rejected promise: a cue is a courtesy, and a
   * courtesy must not be able to break the caller. `reason` says what happened so
   * the UI can explain a silence once instead of leaving the user guessing.
   */
  async play(name: CueName): Promise<PlayResult> {
    if (this.muted) return { played: false, reason: 'muted' };
    const notes = CUES[name];
    if (!notes) return { played: false, reason: 'unsupported' };

    // Coalesce: five approvals resolved at once is one ping, not five.
    const now = this.nowMs();
    if (this.lastCue && this.lastCue.name === name && now - this.lastCue.at < COALESCE_MS) {
      return { played: false, reason: 'coalesced' };
    }

    const context = this.ensureContext();
    if (!context) return { played: false, reason: 'unsupported' };

    if (context.state !== 'running') {
      // Try once from here: a tab that was interacted with earlier resumes fine,
      // and one that was not reports `blocked` so the UI can hint.
      const resumed = await this.arm();
      if (!resumed) return { played: false, reason: 'blocked' };
    }

    try {
      const start = context.currentTime + START_LEAD_S;
      for (const note of notes) {
        this.render(context, note, start);
      }
      this.lastCue = { name, at: now };
      this.scheduleCoalesceClear();
      return { played: true };
    } catch {
      // A context can be closed under us (a page restore, a browser reclaiming
      // memory). Forget it so the next cue rebuilds one.
      this.context = null;
      return { played: false, reason: 'unsupported' };
    }
  }

  /** Release the context. Playback after this rebuilds one. */
  async dispose(): Promise<void> {
    if (this.coalesceTimer !== null) {
      this.env.clearTimeout(this.coalesceTimer);
      this.coalesceTimer = null;
    }
    // A disposed engine has no recent cue: the next play is news again, not a
    // repeat of something played before the context was rebuilt.
    this.lastCue = null;
    this.blocked = false;
    const context = this.context;
    this.context = null;
    if (context) {
      try {
        await context.close();
      } catch {
        // Already closed, or never started: nothing to do.
      }
    }
  }

  /** Render one note with a short attack and an exponential decay. */
  private render(context: AudioContextLike, note: Note, start: number): void {
    const oscillator = context.createOscillator();
    const gain = context.createGain();
    oscillator.type = note.type;
    oscillator.frequency.value = note.hz;
    const peak = Math.max(0.0001, note.peak * this.volume);
    const begin = start + note.at;
    const end = begin + note.duration;

    // A hard on/off clicks. Ramp up over ~8 ms and decay to near-silence: the
    // exponential tail is what makes a synthesized beep sound like a chime.
    if (gain.gain.setValueAtTime) {
      gain.gain.setValueAtTime(0.0001, begin);
      gain.gain.linearRampToValueAtTime?.(peak, begin + ATTACK_S);
      gain.gain.exponentialRampToValueAtTime?.(0.0001, end);
    } else {
      gain.gain.value = peak;
    }

    oscillator.connect(gain);
    gain.connect(context.destination);
    oscillator.start(begin);
    oscillator.stop(end + RELEASE_S);
  }

  private ensureContext(): AudioContextLike | null {
    if (this.context) return this.context;
    if (this.contextUnavailable) return null;
    const context = this.env.createContext();
    if (!context) {
      this.contextUnavailable = true;
      return null;
    }
    this.context = context;
    return context;
  }

  private scheduleCoalesceClear(): void {
    if (this.coalesceTimer !== null) this.env.clearTimeout(this.coalesceTimer);
    this.coalesceTimer = this.env.setTimeout(() => {
      this.coalesceTimer = null;
      this.lastCue = null;
    }, COALESCE_MS);
  }

  private nowMs(): number {
    // `performance.now` where it exists so a test can fake the clock; `Date.now`
    // is the fallback. Only differences matter here.
    if (typeof performance !== 'undefined' && typeof performance.now === 'function') {
      return performance.now();
    }
    return Date.now();
  }
}

/** Window in which a repeated cue of the same name is swallowed. */
export const COALESCE_MS = 400;
/** Small lead so the first note is scheduled in the future, not "now". */
const START_LEAD_S = 0.01;
/** Attack time, long enough to avoid a click. */
const ATTACK_S = 0.008;
/** Extra time before a node is stopped, so the decay is not cut off. */
const RELEASE_S = 0.02;

function clampVolume(volume: number): number {
  if (!Number.isFinite(volume)) return 0.8;
  return Math.min(1, Math.max(0, volume));
}

/** Compute the loudest sample a cue would produce, for tests and diagnostics. */
export function cueNotes(name: CueName): readonly Note[] {
  return CUES[name] ?? [];
}

/** Total length of a cue in seconds. */
export function cueDuration(name: CueName): number {
  const notes = CUES[name] ?? [];
  return notes.reduce((max, note) => Math.max(max, note.at + note.duration), 0);
}

/** The production environment: a real AudioContext, when the browser has one. */
export function browserSoundEnvironment(): SoundEnvironment {
  return {
    createContext: () => {
      // `webkitAudioContext` for Safari < 14.1, which is still in the wild enough
      // to matter for a self-hosted tool.
      const Ctor =
        typeof AudioContext !== 'undefined'
          ? AudioContext
          : (globalThis as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
      if (!Ctor) return null;
      try {
        return new Ctor() as unknown as AudioContextLike;
      } catch {
        // Some browsers throw when too many contexts exist. Silently unsupported.
        return null;
      }
    },
    setTimeout: (handler, ms) => window.setTimeout(handler, ms),
    clearTimeout: (handle) => window.clearTimeout(handle),
  };
}
