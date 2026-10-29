import type { JSX } from 'preact';
import * as actions from '../state/actions';
import { ArrowRightIcon, BoltIcon, LockIcon, SearchIcon, SparkIcon } from './icons';

/** One of the mock's three starter prompts. `prompt` is what lands in the composer. */
export interface QuickPrompt {
  icon: JSX.Element;
  label: string;
  prompt: string;
}

/**
 * The mock's three starters. The label is what the reader sees; the prompt is
 * the complete instruction the composer receives, so nothing has to be typed.
 */
export const QUICK_PROMPTS: readonly QuickPrompt[] = [
  {
    icon: <SearchIcon size={15} />,
    label: 'Explore the codebase and suggest improvements',
    prompt: 'Explore this codebase and suggest the highest-impact improvements.',
  },
  {
    icon: <BoltIcon size={15} />,
    label: 'Find and fix a bug in this project',
    prompt: 'Find and fix a bug in the current project, then run the relevant tests.',
  },
  {
    icon: <LockIcon size={15} />,
    label: 'Review this project for security issues',
    prompt: 'Review this project for security issues and propose fixes.',
  },
];

export interface EmptyStateProps {
  /**
   * `empty` — the open session has no messages yet.
   * `no-session` — nothing is open, so a prompt opens the session first.
   */
  variant?: 'empty' | 'no-session';
}

/**
 * The mock's empty conversation: symbol, "What are we building?", one line of
 * copy and three quick prompts that put a complete instruction in the composer
 * (and focus it). Kept deliberately plain — this is the first thing a new
 * session shows.
 */
export function EmptyState({ variant = 'empty' }: EmptyStateProps): JSX.Element {
  const usePrompt = (prompt: string): void => {
    if (variant === 'no-session') {
      void actions.createSession().then((session) => {
        if (session) actions.fillComposer(prompt);
      });
      return;
    }
    actions.fillComposer(prompt);
  };

  return (
    <section class="empty-state" aria-labelledby="empty-title">
      <div class="empty-content">
        <div class="empty-symbol" aria-hidden="true">
          <SparkIcon size={25} />
        </div>
        <h2 class="empty-title" id="empty-title">
          What are we building?
        </h2>
        <p class="empty-copy">
          Start with an idea or a task. Karucode can explore your codebase, make changes, run tools, and keep
          you in control.
        </p>
        <div class="quick-prompts" aria-label="Suggested prompts">
          {QUICK_PROMPTS.map((quick) => (
            <button
              type="button"
              class="quick-prompt"
              key={quick.label}
              title={quick.prompt}
              onClick={() => usePrompt(quick.prompt)}
            >
              <span class="quick-prompt-icon" aria-hidden="true">
                {quick.icon}
              </span>
              <span>{quick.label}</span>
              <ArrowRightIcon size={14} />
            </button>
          ))}
        </div>
      </div>
    </section>
  );
}
