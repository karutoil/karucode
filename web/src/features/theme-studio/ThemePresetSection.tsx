/* Theme library: the preset gallery and the inline "save as new" form.
 *
 * A preset is a selectable card, not a `Button` variant: `Button`'s fixed 30px
 * height, `inline-flex` and `whitespace-nowrap` would flatten the 62px
 * miniature and the two-line meta that `.theme-preset` in `theme-studio.css`
 * owns (and `styles/theme-studio.test.ts` pins), so the card stays a semantic
 * `button[aria-pressed]` and the sheet keeps painting it.
 */

import type { JSX, RefObject } from 'preact';
import { useId } from 'preact/hooks';
import { activeTheme, customThemes, draftEdited, selectTheme, themeLibrary } from '../../theme';
import { Button, Input, LinkButton, Text, icons } from '../../ui';
import { MODE_LABELS, presetStyle } from './helpers';

export interface ThemePresetSectionProps {
  naming: boolean;
  nameValue: string;
  nameInput: RefObject<HTMLInputElement | null>;
  onNameInput: (value: string) => void;
  onOpenNaming: () => void;
  onCancelNaming: () => void;
  /** Close the form silently — a preset was chosen, so focus stays there. */
  onDismissNaming: () => void;
  onSubmitName: (event: Event) => void;
}

export function ThemePresetSection({
  naming,
  nameValue,
  nameInput,
  onNameInput,
  onOpenNaming,
  onCancelNaming,
  onDismissNaming,
  onSubmitName,
}: ThemePresetSectionProps): JSX.Element {
  const headingId = useId();
  const nameId = useId();
  const nameHintId = useId();

  const theme = activeTheme.value;
  const library = themeLibrary.value;
  const customs = customThemes.value;
  const dirty = draftEdited.value;

  return (
    <section class="theme-section" aria-labelledby={headingId}>
      <div class="theme-section-title">
        <Text as="h3" id={headingId} size="label" weight="bold">
          Theme library
        </Text>
        <LinkButton tone="accent" class="theme-mini-action" onClick={onOpenNaming}>
          <icons.PlusIcon size={13} />
          New theme
        </LinkButton>
      </div>
      <Text as="p" class="theme-description" size="tiny" tone="muted">
        Pick a preset, then adjust anything below. Changes apply as you make them.
      </Text>

      {naming ? (
        <form class="theme-inline-create" onSubmit={onSubmitName}>
          <label class="sr-only" for={nameId}>
            Theme name
          </label>
          <Input
            id={nameId}
            inputRef={nameInput}
            class="theme-name-input"
            type="text"
            value={nameValue}
            maxLength={50}
            placeholder="My custom theme"
            autoComplete="off"
            aria-describedby={nameHintId}
            testId="theme-name-input"
            onInput={(event) => onNameInput(event.currentTarget.value)}
          />
          <Button
            variant="primary"
            type="submit"
            class="theme-btn primary"
            icon={<icons.CheckIcon size={14} />}
            testId="theme-save-confirm"
          >
            Save theme
          </Button>
          <Button class="theme-btn" onClick={onCancelNaming}>
            Cancel
          </Button>
          <span class="sr-only" id={nameHintId}>
            Saves the current colours and interface settings as a new custom theme.
          </span>
        </form>
      ) : null}

      <ul class="theme-gallery" role="list">
        {library.map((entry) => {
          const selected = !dirty && entry.id === theme.id;
          const custom = customs.some((candidate) => candidate.id === entry.id);
          const modeLabel = MODE_LABELS[entry.mode];
          return (
            <li key={entry.id}>
              <button
                type="button"
                class={selected ? 'theme-preset selected' : 'theme-preset'}
                style={presetStyle(entry)}
                aria-pressed={selected}
                // The preset name and its mode are adjacent spans, so the
                // content-derived accessible name would run them together
                // ("EmberDark"). Name it as it is announced.
                aria-label={`${entry.name} ${modeLabel}${custom ? ' · Custom' : ''}`}
                title={entry.description ?? entry.name}
                data-testid={`theme-preset-${entry.id}`}
                onClick={() => {
                  onDismissNaming();
                  selectTheme(entry.id);
                }}
              >
                <span class="theme-preset-demo" aria-hidden="true">
                  <span class="theme-mini-rail" />
                  <span class="theme-mini-sidebar">
                    <i />
                    <i />
                    <i />
                  </span>
                  <span class="theme-mini-main">
                    <i />
                    <i />
                    <i />
                  </span>
                  <span class="theme-mini-accent" />
                </span>
                <span class="theme-preset-meta">
                  <Text as="span" class="theme-preset-name" size="small" weight="semibold">
                    {entry.name}
                  </Text>
                  <Text as="span" class="theme-preset-mode" size="tiny" tone="muted" mono>
                    {modeLabel}
                    {custom ? ' · Custom' : ''}
                  </Text>
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
