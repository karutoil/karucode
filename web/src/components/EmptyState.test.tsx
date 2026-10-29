import type { JSX } from 'preact';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { EmptyState, QUICK_PROMPTS } from './EmptyState';
import { Composer } from './Composer';
import * as actions from '../state/actions';
import * as store from '../state/store';
import type { Activity } from '../state/selectors';

// Only `createSession` is replaced: the real `fillComposer` is what the prompts
// must exercise, and the no-session path needs a session before it can fill.
vi.mock('../state/actions', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../state/actions')>();
  return {
    ...actual,
    createSession: vi.fn(async () => ({ id: 's_new', title: 'New session' }) as never),
  };
});

const IDLE: Activity = { kind: 'idle', label: 'Ready' };

const EXPLORE = 'Explore this codebase and suggest the highest-impact improvements.';
const REVIEW = 'Review this project for security issues and propose fixes.';

/** The empty state and the composer share one view in the real app. */
function Harness({ variant = 'empty' }: { variant?: 'empty' | 'no-session' }): JSX.Element {
  return (
    <>
      <EmptyState variant={variant} />
      <Composer
        disabledReason={null}
        running={false}
        stopping={false}
        status={IDLE}
        onSend={async () => true}
        onStop={() => undefined}
        onOpenSettings={() => undefined}
        focusTick={store.composerFocusTick.value}
      />
    </>
  );
}

function textarea(): HTMLTextAreaElement {
  return screen.getByLabelText('Message karucode') as HTMLTextAreaElement;
}

function prompt(label: RegExp): HTMLElement {
  return screen.getByRole('button', { name: label });
}

beforeEach(() => {
  store.composerDraft.value = null;
  store.composerFocusTick.value = 0;
});

afterEach(() => {
  store.composerDraft.value = null;
  store.composerFocusTick.value = 0;
});

describe('EmptyState', () => {
  it('renders the mock empty state verbatim', () => {
    render(<EmptyState />);

    expect(screen.getByRole('heading', { name: 'What are we building?' })).not.toBeNull();
    expect(
      screen.getByText(
        'Start with an idea or a task. Karucode can explore your codebase, make changes, run tools, and keep you in control.',
      ),
    ).not.toBeNull();
    expect(document.querySelector('.empty-symbol')).not.toBeNull();
    expect(document.querySelector('.empty-content')).not.toBeNull();
  });

  it('offers the three mock quick prompts in the mock order', () => {
    render(<EmptyState />);

    const buttons = screen.getAllByRole('button');
    expect(buttons).toHaveLength(3);
    expect(buttons.map((button) => button.textContent?.trim())).toEqual([
      'Explore the codebase and suggest improvements',
      'Find and fix a bug in this project',
      'Review this project for security issues',
    ]);
  });

  it('exposes the full instruction of each prompt as a tooltip', () => {
    render(<EmptyState />);
    for (const quick of QUICK_PROMPTS) {
      expect(screen.getByRole('button', { name: quick.label }).getAttribute('title')).toBe(quick.prompt);
    }
  });

  it('fills the composer with a whole instruction so nothing has to be typed', () => {
    render(<Harness />);
    expect(textarea().value).toBe('');

    fireEvent.click(prompt(/Explore the codebase/));

    expect(textarea().value).toBe(EXPLORE);
    expect(document.activeElement).toBe(textarea());
    // The hand-off is one-shot: the signal is cleared so a remount cannot replay it.
    expect(store.composerDraft.value).toBeNull();
  });

  it('hands over each prompt exactly as the mock defines it', () => {
    render(<Harness />);
    for (const quick of QUICK_PROMPTS) {
      fireEvent.click(screen.getByRole('button', { name: quick.label }));
      expect(textarea().value).toBe(quick.prompt);
    }
  });

  it('is keyboard reachable: prompts are buttons in a labelled group', () => {
    render(<EmptyState />);

    expect(document.querySelector('.quick-prompts')?.getAttribute('aria-label')).toBe('Suggested prompts');
    const first = prompt(/Explore the codebase/) as HTMLButtonElement;
    expect(first.type).toBe('button');

    first.focus();
    expect(document.activeElement).toBe(first);
    fireEvent.click(first);
    expect(store.composerDraft.value).toBe(EXPLORE);
  });

  it('does not create a session just by rendering the empty state', () => {
    render(<Harness />);
    expect(vi.mocked(actions.createSession)).not.toHaveBeenCalled();
  });

  it('opens a session before filling the composer when nothing is open', async () => {
    render(<Harness variant="no-session" />);
    fireEvent.click(prompt(/Review this project/));

    await vi.waitFor(() => expect(vi.mocked(actions.createSession)).toHaveBeenCalledTimes(1));
    await vi.waitFor(() => expect(textarea().value).toBe(REVIEW));
  });
});
