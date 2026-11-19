import type { JSX, TargetedKeyboardEvent } from 'preact';
import { useRef } from 'preact/hooks';
import type { QueuedKind, QueuedMessage } from '../../api/types';
import { Badge, Button, Inline, Stack, Text } from '../../ui';
import { FileIcon } from '../../components/icons';
import { QUEUE_LABEL, QUEUE_TITLE } from './helpers';

/**
 * Which queue a message typed while the agent works goes into.
 *
 * Shown only while a run is in flight, because that is the only time the choice
 * exists: with nothing running, sending *is* the follow-up. The two options are
 * genuinely different - a steer changes the turn in progress, a follow-up begins
 * the next one - so the toggle says which one is armed rather than hiding the
 * choice behind a modifier key.
 *
 * Keyboard behaviour is the ARIA radio-group contract, which a row of plain
 * buttons does not give you: exactly one option is in the tab order (the checked
 * one), Left/Up and Right/Down move *and* select the neighbour, Home/End jump to
 * the ends, and Space/Enter activate the focused option natively. Without the
 * roving tabindex a keyboard user had to tab through both options, and the arrow
 * keys did nothing, so the control announced itself as a radiogroup and then
 * refused to behave like one.
 *
 * The two options stay native `<button role="radio">`s rather than `Button`s:
 * they are 24px tall inside a 30px capsule whose geometry, inset and
 * `[data-active]` pill are owned by `composer.css`, and `Button` always emits a
 * 26/30px height plus its own colour utilities - utilities outrank the component
 * sheet, so the capsule would be overrun instead of reused.
 */
export function QueueModeToggle({
  mode,
  onChange,
}: {
  mode: QueuedKind;
  onChange: (next: QueuedKind) => void;
}): JSX.Element {
  const kinds = Object.keys(QUEUE_LABEL) as QueuedKind[];
  const buttons = useRef(new Map<QueuedKind, HTMLButtonElement | null>());

  /** Select an option and move focus to it, like a native radio button. */
  const choose = (next: QueuedKind): void => {
    onChange(next);
    buttons.current.get(next)?.focus();
  };

  const onKeyDown = (event: TargetedKeyboardEvent<HTMLButtonElement>): void => {
    const index = kinds.indexOf(mode);
    let next: QueuedKind | undefined;
    if (event.key === 'ArrowRight' || event.key === 'ArrowDown') {
      next = kinds[(index + 1) % kinds.length];
    } else if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') {
      next = kinds[(index - 1 + kinds.length) % kinds.length];
    } else if (event.key === 'Home') {
      next = kinds[0];
    } else if (event.key === 'End') {
      next = kinds[kinds.length - 1];
    }
    if (next === undefined) return;
    // Both keys would scroll the transcript otherwise.
    event.preventDefault();
    choose(next);
  };

  return (
    <Inline
      class="queue-mode"
      gap="1"
      align="center"
      role="radiogroup"
      aria-label="What this message does"
      testId="queue-mode"
    >
      {kinds.map((kind) => (
        <button
          key={kind}
          ref={(element) => {
            buttons.current.set(kind, element);
          }}
          type="button"
          role="radio"
          aria-checked={mode === kind}
          // The checked option is the group's one tab stop; arrows move within.
          tabIndex={mode === kind ? 0 : -1}
          class="queue-mode-option"
          data-active={mode === kind ? 'true' : 'false'}
          title={QUEUE_TITLE[kind]}
          onClick={() => onChange(kind)}
          onKeyDown={onKeyDown}
        >
          {QUEUE_LABEL[kind]}
        </button>
      ))}
    </Inline>
  );
}

/**
 * The messages waiting for this run, with the one action that matters on each:
 * take it back.
 *
 * A queued message is invisible in the transcript by design (a steer only becomes
 * a turn when the loop folds it in), which made a mis-sent message impossible to
 * undo before this panel existed. Recall is honest about the race: the server
 * answers `404` if the run already applied the message, and the row says the same
 * thing rather than pretending the click always works.
 */
export function QueuedPanel({
  queued,
  recalling,
  onRecall,
}: {
  queued: readonly QueuedMessage[];
  recalling: Record<string, boolean>;
  onRecall: (queuedId: string) => void;
}): JSX.Element | null {
  if (queued.length === 0) return null;
  return (
    <Stack
      as="section"
      // No gap: the panel's own padding and the heading's margin already space
      // the two bands, and `.queued-panel` is a padded block, not a stack.
      gap="0"
      class="queued-panel"
      aria-label="Queued messages"
      testId="queued-panel"
    >
      <Text as="p" size="micro" tone="muted" weight="semibold" class="queued-head">
        {queued.length === 1 ? '1 message waiting' : `${queued.length} messages waiting`}
        <Text as="span" size="micro" tone="muted" weight="medium" class="queued-head-hint">
          {' · the agent sees them in this order'}
        </Text>
      </Text>
      <Stack as="ul" gap="2" class="queued-list">
        {queued.map((entry) => (
          <li key={entry.id} class="queued-row" data-kind={entry.kind}>
            {/* The tone carries the one distinction the stylesheet used to make
                by hand: a steer acts on this turn, a follow-up waits its own. */}
            <Badge
              tone={entry.kind === 'follow_up' ? 'neutral' : 'accent'}
              class="queued-kind"
              title={QUEUE_TITLE[entry.kind]}
              testId={`queued-kind-${entry.id}`}
            >
              {QUEUE_LABEL[entry.kind]}
            </Badge>
            <Stack gap="2" class="queued-body">
              <Text as="span" size="tiny" class="queued-text" title={entry.content}>
                {entry.draft ?? entry.content}
              </Text>
              {entry.attachments && entry.attachments.length > 0 ? (
                <Inline gap="2" wrap class="queued-attachments">
                  {entry.attachments.map((attachment) => (
                    <Text
                      key={`${entry.id}:${attachment.path}`}
                      as="span"
                      size="micro"
                      tone="muted"
                      mono
                      class="queued-attachment"
                      title={attachment.path}
                    >
                      <FileIcon size={10} />
                      {attachment.name}
                    </Text>
                  ))}
                </Inline>
              ) : null}
            </Stack>
            <Button
              variant="secondary"
              size="sm"
              class="queued-recall"
              disabled={recalling[entry.id] === true}
              onClick={() => onRecall(entry.id)}
              aria-label={`Recall ${QUEUE_LABEL[entry.kind].toLowerCase()}: ${entry.draft ?? entry.content}`}
              title="Take this back, with its attachments, and put it in the composer"
              testId={`queued-recall-${entry.id}`}
            >
              {recalling[entry.id] ? 'Recalling…' : 'Recall'}
            </Button>
          </li>
        ))}
      </Stack>
    </Stack>
  );
}
