import type { JSX } from 'preact';
import { memo } from 'preact/compat';
import type { SlashItem } from '../state/chat';
import { Markdown } from './Markdown';

/**
 * One slash command's answer in the transcript.
 *
 * Drawn as the reader's own action rather than as a model turn: the command is on
 * the card's face, the body is the markdown the server wrote, and a refusal is
 * tinted like a warning so `/compact` telling you a run is in flight cannot be
 * mistaken for `/compact` having happened.
 *
 * The card is deliberately quiet. A command result is a receipt, not a message -
 * it should be skimmable above and below the conversation it sits in, so nothing
 * here is a button, an icon or an animation.
 */
function SlashCardImpl({ item }: { item: SlashItem }): JSX.Element {
  return (
    <article
      class={`slash-card${item.ok ? '' : ' is-error'}`}
      data-command={item.command}
      aria-label={`/${item.command}${item.ok ? '' : ' (refused)'}`}
    >
      <header class="slash-card-head">
        <code class="slash-card-command">/{item.command}</code>
        {item.ok ? null : <span class="slash-card-state">refused</span>}
      </header>
      {item.content.trim() !== '' ? (
        <div class="slash-card-body">
          <Markdown text={item.content} />
        </div>
      ) : null}
    </article>
  );
}

/** Memoised like the other transcript rows: a command result never changes. */
export const SlashCard = memo(SlashCardImpl);