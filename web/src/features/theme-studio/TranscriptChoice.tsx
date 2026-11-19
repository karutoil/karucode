/* A segmented transcript option, and the miniature it repaints.
 *
 * The control is a `role="group"` row of pressed buttons: `aria-pressed` says
 * which one wins, and the hint under the row says what the winner does — "Turns"
 * on its own does not say what gets folded.
 *
 * The miniature is a picture, not a control: it is drawn from the live theme
 * tokens by `theme-studio.css`, so choosing Rows or Cards shows the shape of the
 * result instead of asking the reader to imagine it.
 */

import type { JSX } from 'preact';
import { useId } from 'preact/hooks';
import { activeTheme, type ThemeChoice } from '../../theme';
import { Button, Inline, Text } from '../../ui';

export interface TranscriptChoiceProps<T extends string> {
  label: string;
  value: T;
  choices: readonly ThemeChoice<T>[];
  onChange: (next: T) => void;
  testId: string;
}

export function TranscriptChoice<T extends string>({
  label,
  value,
  choices,
  onChange,
  testId,
}: TranscriptChoiceProps<T>): JSX.Element {
  const labelId = useId();
  const hintId = useId();
  const selected = choices.find((choice) => choice.value === value);

  return (
    <div class="theme-field">
      <Text
        as="span"
        id={labelId}
        class="theme-field-label"
        size="tiny"
        tone="secondary"
        weight="semibold"
      >
        {label}
      </Text>
      <Inline
        gap="2"
        role="group"
        class="theme-segments"
        aria-labelledby={labelId}
        aria-describedby={hintId}
        testId={testId}
      >
        {choices.map((choice) => (
          <Button
            key={choice.value}
            size="sm"
            variant={choice.value === value ? 'secondary' : 'ghost'}
            class={choice.value === value ? 'theme-segment selected' : 'theme-segment'}
            aria-pressed={choice.value === value}
            title={choice.hint}
            testId={`${testId}-${choice.value}`}
            onClick={() => onChange(choice.value)}
          >
            {choice.label}
          </Button>
        ))}
      </Inline>
      <Text as="p" id={hintId} class="theme-field-hint" size="tiny" tone="muted">
        {selected?.hint ?? ''}
      </Text>
    </div>
  );
}

/** A miniature of the real transcript, repainted by these very options. */
export function TranscriptPreview(): JSX.Element {
  const theme = activeTheme.value;
  return (
    <div class="theme-preview" data-testid="theme-transcript-preview" aria-hidden="true">
      <p
        class="theme-preview-narration"
        data-on={theme.grouping === 'turns' && theme.working === 'folded' ? 'folded' : 'shown'}
      >
        Reading the transcript layout…
      </p>
      <div class="theme-preview-row" hidden={theme.toolStyle === 'cards'}>
        <span class="theme-preview-dot" />
        <span class="theme-preview-name">read_file</span>
        <span class="theme-preview-args">src/theme/types.ts</span>
        <span class="theme-preview-meta">12ms</span>
      </div>
      <div class="theme-preview-card" hidden={theme.toolStyle === 'rows'}>
        <span class="theme-preview-card-head">
          <span class="theme-preview-dot" />
          <span class="theme-preview-name">read_file</span>
          <span class="theme-preview-args">src/theme/types.ts</span>
        </span>
        <span class="theme-preview-card-body">export type ThemeToolStyle = …</span>
      </div>
      <p class={`theme-preview-thought${theme.thinking === 'collapsed' ? ' is-collapsed' : ''}`}>
        Thought{theme.thinking === 'preview' ? ' · The view options belong on the theme.' : ''}
      </p>
    </div>
  );
}
