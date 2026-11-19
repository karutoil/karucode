import type { JSX } from 'preact';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Button } from '../ui/Button';
import { Text } from '../ui/Layout';
import { ArrowRightIcon, BoltIcon, LockIcon, SearchIcon, SparkIcon } from './icons';
import { pluginText, UI_REPLACE_GREETING } from './uiContributions';

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
   * `empty` — the open session has no messages yet, so a prompt fills the box.
   * `no-session` — nothing is open, so a prompt opens a draft conversation first.
   */
  variant?: 'empty' | 'no-session';
}

/**
 * The mock's empty conversation: symbol, "What are we building?", one line of
 * copy and three quick prompts that put a complete instruction in the composer
 * (and focus it). Kept deliberately plain — this is the first thing a new
 * session shows.
 *
 * The heading is a replaceable target (`empty.greeting`): a deployment that
 * renamed the product can welcome its people in its own words instead.
 */
export function EmptyState({ variant = 'empty' }: EmptyStateProps): JSX.Element {
  const usePrompt = (prompt: string): void => {
    // A starter prompt begins a conversation, so with nothing open it opens a
    // draft first - the same draft "New session" starts - and only the send
    // creates a session row. Nothing is written to the server here.
    if (variant === 'no-session') actions.startDraftSession();
    actions.fillComposer(prompt);
  };

  const greeting = pluginText(store.pluginUi.value, UI_REPLACE_GREETING, 'What are we building?');

  return (
    <section class="empty-state" aria-labelledby="empty-title">
      <div class="empty-content">
        <div class="empty-symbol" aria-hidden="true">
          <SparkIcon size={25} />
        </div>
        <Text as="h2" id="empty-title" testId="empty-title" size="hero" weight="bold" class="empty-title">
          {greeting}
        </Text>
        {/* `size`/`tone` are the values `.empty-copy` already states
            (`--fs-message`, `--muted`): the sentence keeps the quiet voice it was
            given, and the type scale stops being copied into this file. */}
        <Text as="p" size="message" tone="muted" class="empty-copy">
          Start with an idea or a task. Karucode can explore your codebase, make changes, run tools, and keep
          you in control.
        </Text>
        <div class="quick-prompts" aria-label="Suggested prompts">
          {QUICK_PROMPTS.map((quick, index) => (
            <Button
              class="quick-prompt h-auto justify-start"
              key={quick.label}
              title={quick.prompt}
              testId={`quick-prompt-${index}`}
              onClick={() => usePrompt(quick.prompt)}
            >
              <span class="quick-prompt-icon" aria-hidden="true">
                {quick.icon}
              </span>
              {/* A starter prompt is a sentence in a row that hugs its content,
                  while `Button` is a one-line control: `h-auto justify-start`
                  keep the row from being flattened onto the 30px button height,
                  and the label opts out of the inherited `nowrap` itself (the
                  property is inherited, so the row keeps its own). */}
              <span class="whitespace-normal">{quick.label}</span>
              <ArrowRightIcon size={14} />
            </Button>
          ))}
        </div>
      </div>
    </section>
  );
}
