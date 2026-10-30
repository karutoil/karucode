/**
 * The sound engine, against a fake AudioContext.
 *
 * jsdom has no Web Audio (`typeof AudioContext === 'undefined'`), so the engine
 * takes its context and its timers from the outside. What these tests can prove is
 * every *decision*: which cue for which kind, mute, volume, coalescing, and the
 * three failure modes. What they cannot prove is that a cue is audible — that is
 * done in a real browser with `OfflineAudioContext`, which is the only place the
 * rendered samples exist.
 */

import { describe, expect, it, vi } from 'vitest';
import {
  CUE_LABELS,
  CUE_NAMES,
  COALESCE_MS,
  SoundEngine,
  browserSoundEnvironment,
  cueDuration,
  cueNotes,
  type AudioContextLike,
  type CueName,
  type SoundEnvironment,
} from './notifySound';

/** `notes[i]` from the cue table, asserting it exists. */
function cueNote(name: CueName, index: number) {
  const note = cueNotes(name)[index];
  if (!note) throw new Error(`cue ${name} has no note ${index}`);
  return note;
}

/** `notes[i]`, asserting it exists: a missing note is the failure being tested. */
function nth(notes: StartedNote[], index: number): StartedNote {
  const note = notes[index];
  if (!note) throw new Error(`expected at least ${index + 1} notes, got ${notes.length}`);
  return note;
}

interface StartedNote {
  type: OscillatorType;
  hz: number;
  startAt: number;
  stopAt: number;
  peak: number;
}

/** A recording fake: everything the engine does is inspectable afterwards. */
function fakeEnvironment(state: string = 'running'): {
  env: SoundEnvironment;
  notes: StartedNote[];
  contexts: number;
  resumeCalls: number;
  closed: number;
  setState: (state: string) => void;
  failCreate: () => void;
  failResume: () => void;
} {
  const notes: StartedNote[] = [];
  /** The note whose gain has not been created yet (engine order: osc, then gain). */
  let pendingNote: StartedNote | null = null;
  let currentState = state;
  let created = 0;
  let resumeCalls = 0;
  let closed = 0;
  let createFails = false;
  let resumeFails = false;

  const makeContext = (): AudioContextLike => {
    created += 1;
    let currentTime = 0;
    return {
      get state() {
        return currentState;
      },
      get currentTime() {
        // Advance slightly per read so scheduled times are distinguishable.
        currentTime += 0.001;
        return currentTime;
      },
      destination: {},
      resume: async () => {
        resumeCalls += 1;
        if (resumeFails) throw new Error('NotAllowedError');
        currentState = 'running';
      },
      close: async () => {
        closed += 1;
        currentState = 'closed';
      },
      createOscillator: () => {
        const note: StartedNote = {
          type: 'sine',
          hz: 0,
          startAt: 0,
          stopAt: 0,
          peak: 0,
        };
        // The engine creates the oscillator first and its gain second, so the gain
        // records the envelope onto this note.
        pendingNote = note;
        const oscillator = {
          set type(value: OscillatorType) {
            note.type = value;
          },
          get type() {
            return note.type;
          },
          frequency: {
            set value(value: number) {
              note.hz = value;
            },
            get value() {
              return note.hz;
            },
          },
          connect: () => undefined,
          start: (when?: number) => {
            note.startAt = when ?? 0;
            notes.push(note);
          },
          stop: (when?: number) => {
            note.stopAt = when ?? 0;
          },
        };
        return oscillator;
      },
      createGain: () => {
        // The engine schedules the attack on the gain it just created, so the peak
        // belongs to whichever oscillator is created next (the engine always pairs
        // one gain with one oscillator, in that order).
        const pending = pendingNote;
        return {
          gain: {
            value: 1,
            setValueAtTime: () => undefined,
            linearRampToValueAtTime: (value: number) => {
              // The largest ramp target in an envelope is the peak, so the attack
              // ramp wins over the decay's near-zero target.
              if (pending) pending.peak = Math.max(pending.peak, value);
            },
            exponentialRampToValueAtTime: () => undefined,
          },
          connect: () => undefined,
        };
      },
    };
  };

  const env: SoundEnvironment = {
    createContext: () => {
      if (createFails) return null;
      return makeContext();
    },
    setTimeout: (handler, ms) => {
      const handle = setTimeout(handler, ms);
      return handle as unknown as number;
    },
    clearTimeout: (handle) => clearTimeout(handle as unknown as ReturnType<typeof setTimeout>),
  };

  return {
    env,
    notes,
    get contexts() {
      return created;
    },
    get resumeCalls() {
      return resumeCalls;
    },
    get closed() {
      return closed;
    },
    setState: (next) => {
      currentState = next;
    },
    failCreate: () => {
      createFails = true;
    },
    failResume: () => {
      resumeFails = true;
    },
  };
}

describe('cue table', () => {
  it('defines exactly the four documented cues', () => {
    expect(CUE_NAMES).toEqual(['finished', 'attention', 'error', 'tick']);
    for (const name of CUE_NAMES) {
      expect(cueNotes(name).length).toBeGreaterThan(0);
      expect(CUE_LABELS[name]).toBeTruthy();
    }
  });

  it('gives each cue a distinct shape, not only a distinct pitch', () => {
    // A cue must be recognisable on a laptop speaker, so the *number* of notes
    // differs between the ones a user hears most.
    expect(cueNotes('finished')).toHaveLength(2);
    expect(cueNotes('attention')).toHaveLength(3);
    expect(cueNotes('error')).toHaveLength(2);
    expect(cueNotes('tick')).toHaveLength(1);
    // ...and the oscillator shapes differ too: error is dull, not bright.
    expect(cueNote('error', 0).type).toBe('triangle');
    expect(cueNote('finished', 0).type).toBe('sine');
  });

  it('rises for finished and repeats for attention', () => {
    expect(cueNote('finished', 1).hz).toBeGreaterThan(cueNote('finished', 0).hz);
    // The first two notes of the attention cue are the same note twice: that
    // repetition is what makes it read as "look at me" rather than "done".
    expect(cueNote('attention', 0).hz).toBe(cueNote('attention', 1).hz);
  });

  it('keeps every cue short and quiet', () => {
    for (const name of CUE_NAMES) {
      expect(cueDuration(name)).toBeLessThanOrEqual(1);
      for (const note of cueNotes(name)) {
        // A notification that startles is a notification people turn off.
        expect(note.peak).toBeLessThanOrEqual(0.25);
      }
    }
  });
});

describe('playing a cue', () => {
  it('renders every note of the requested cue', async () => {
    const fake = fakeEnvironment();
    const engine = new SoundEngine(fake.env);
    const result = await engine.play('finished');
    expect(result).toEqual({ played: true });
    expect(fake.notes).toHaveLength(2);
    expect(nth(fake.notes, 0).hz).toBe(660);
    expect(nth(fake.notes, 1).hz).toBe(880);
    // Every note is started and then stopped, or it would sound forever.
    for (const note of fake.notes) {
      expect(note.stopAt).toBeGreaterThan(note.startAt);
    }
  });

  it('is silent when muted, and never touches the audio context', async () => {
    const fake = fakeEnvironment();
    const engine = new SoundEngine(fake.env, { muted: true });
    expect(await engine.play('finished')).toEqual({ played: false, reason: 'muted' });
    expect(fake.notes).toHaveLength(0);
    // Muted means "do not create a context at all": a suspended context is a
    // resource the page does not need.
    expect(fake.contexts).toBe(0);
  });

  it('scales the peak with the volume', async () => {
    const loud = fakeEnvironment();
    const quiet = fakeEnvironment();
    await new SoundEngine(loud.env, { volume: 1 }).play('tick');
    await new SoundEngine(quiet.env, { volume: 0.25 }).play('tick');
    expect(nth(loud.notes, 0).peak).toBeGreaterThan(nth(quiet.notes, 0).peak);
  });

  it('clamps a nonsense volume instead of going silent', async () => {
    const fake = fakeEnvironment();
    const engine = new SoundEngine(fake.env, { volume: 12 });
    expect(engine.getVolume()).toBe(1);
    engine.setVolume(-3);
    expect(engine.getVolume()).toBe(0);
  });

  it('coalesces a burst of the same cue into one', async () => {
    const fake = fakeEnvironment();
    const engine = new SoundEngine(fake.env);
    expect((await engine.play('attention')).played).toBe(true);
    // Five approvals resolved together must be one ping, not five.
    expect((await engine.play('attention')).reason).toBe('coalesced');
    expect((await engine.play('attention')).reason).toBe('coalesced');
    expect(fake.notes.filter((n) => n.hz === 880)).toHaveLength(2);
  });

  it('does not coalesce two different cues', async () => {
    const fake = fakeEnvironment();
    const engine = new SoundEngine(fake.env);
    expect((await engine.play('finished')).played).toBe(true);
    expect((await engine.play('error')).played).toBe(true);
  });

  it('plays the same cue again once the coalesce window has passed', async () => {
    vi.useFakeTimers();
    try {
      const fake = fakeEnvironment();
      const engine = new SoundEngine(fake.env);
      expect((await engine.play('finished')).played).toBe(true);
      vi.advanceTimersByTime(COALESCE_MS + 10);
      expect((await engine.play('finished')).played).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('everything a browser can refuse', () => {
  it('reports unsupported when the browser has no Web Audio', async () => {
    const fake = fakeEnvironment();
    fake.failCreate();
    const engine = new SoundEngine(fake.env);
    expect(await engine.play('finished')).toEqual({
      played: false,
      reason: 'unsupported',
    });
    expect(engine.status()).toBe('unsupported');
    // Asking twice must not build a context twice: the first answer is cached.
    expect(await engine.play('error')).toEqual({
      played: false,
      reason: 'unsupported',
    });
    expect(fake.contexts).toBe(0);
  });

  it('reports blocked, not an exception, when the context will not resume', async () => {
    const fake = fakeEnvironment('suspended');
    fake.failResume();
    const engine = new SoundEngine(fake.env);
    const result = await engine.play('finished');
    expect(result).toEqual({ played: false, reason: 'blocked' });
    // The caller needs to be able to tell the user why, once.
    expect(engine.status()).toBe('blocked');
    expect(fake.notes).toHaveLength(0);
  });

  it('resumes a suspended context on its own when the page has been interacted with', async () => {
    // This is the common case: the tab was used, so `resume()` succeeds even
    // though the context starts suspended.
    const fake = fakeEnvironment('suspended');
    const engine = new SoundEngine(fake.env);
    const result = await engine.play('finished');
    expect(result).toEqual({ played: true });
    expect(fake.resumeCalls).toBe(1);
  });

  it('arm() resumes from a gesture and is idempotent', async () => {
    const fake = fakeEnvironment('suspended');
    const engine = new SoundEngine(fake.env);
    expect(await engine.arm()).toBe(true);
    expect(await engine.arm()).toBe(true);
    // Only the first arm needed to resume; the second found it running.
    expect(fake.resumeCalls).toBe(1);
    expect(engine.status()).toBe('ready');
  });

  it('never throws, whatever the browser does', async () => {
    const fake = fakeEnvironment();
    fake.failCreate();
    const engine = new SoundEngine(fake.env);
    // The contract is a resolved PlayResult, always: a cue is a courtesy and a
    // courtesy must not break its caller.
    await expect(engine.play('error')).resolves.toMatchObject({ played: false });
  });

  it('rebuilds a context that was closed under it', async () => {
    const fake = fakeEnvironment();
    const engine = new SoundEngine(fake.env);
    await engine.play('tick');
    const first = fake.contexts;
    await engine.dispose();
    await engine.play('tick');
    expect(fake.contexts).toBeGreaterThan(first);
  });
});

describe('the production environment', () => {
  it('reports no context when the browser has none (jsdom)', () => {
    // jsdom is exactly the case that matters: `createContext` returns null rather
    // than throwing, so a host without Web Audio gets silence, not a crash.
    const env = browserSoundEnvironment();
    expect(env.createContext()).toBeNull();
  });
});

describe('the cue a kind maps to', () => {
  // The mapping lives in the notifier, but the *set* of names is this module's
  // contract, so pin it here: a typo in the notifier would otherwise fall back to
  // silence.
  it('has a name for every cue the notifier can ask for', () => {
    const expected: CueName[] = ['finished', 'attention', 'error', 'tick'];
    for (const name of expected) {
      expect(CUE_NAMES).toContain(name);
    }
  });
});
