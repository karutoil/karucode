import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import * as THEME from '../../theme';
import type { ThemeDensity, ThemeToolStyle, ThemeThinking } from '../../theme';
import { Button, Inline, Stack, Text } from '../../ui';

const TOOL_STYLES: readonly { value: ThemeToolStyle; label: string }[] = [
  { value: 'rows', label: 'Rows' },
  { value: 'cards', label: 'Cards' },
];

const DENSITIES: readonly { value: ThemeDensity; label: string }[] = [
  { value: 'compact', label: 'Compact' },
  { value: 'comfortable', label: 'Comfortable' },
  { value: 'spacious', label: 'Spacious' },
];

const THINKING: readonly { value: ThemeThinking; label: string }[] = [
  { value: 'collapsed', label: 'Collapsed' },
  { value: 'preview', label: 'Preview' },
  { value: 'expanded', label: 'Expanded' },
];

export interface SegmentProps<T extends string> {
  label: string;
  /** The accessible name of the group, when the visible label is not enough. */
  groupLabel: string;
  value: T;
  choices: readonly { value: T; label: string }[];
  hint?: string;
  onChange: (value: T) => void;
}

/** One labelled row of mutually exclusive buttons. */
function Segment<T extends string>({
  label,
  groupLabel,
  value,
  choices,
  hint,
  onChange,
}: SegmentProps<T>): JSX.Element {
  return (
    <Stack gap="2">
      <Text as="span" size="label" weight="semibold">
        {label}
      </Text>
      <Inline gap="2" role="group" aria-label={groupLabel} wrap>
        {choices.map((choice) => (
          <Button
            key={choice.value}
            size="sm"
            variant={choice.value === value ? 'primary' : 'secondary'}
            aria-pressed={choice.value === value}
            onClick={() => onChange(choice.value)}
          >
            {choice.label}
          </Button>
        ))}
      </Inline>
      {hint ? (
        <Text as="p" size="tiny" tone="muted">
          {hint}
        </Text>
      ) : null}
    </Stack>
  );
}

/**
 * Step 3: the look.
 *
 * Self-contained on purpose: every choice here writes straight through the theme
 * engine, which is the same engine the settings sheet and the theme studio use,
 * and the draft state exists only to re-render the highlight. Nothing on this
 * step is part of the server config, so the wizard's save has nothing to do.
 */
export function AppearanceStep(): JSX.Element {
  const [toolStyle, setToolStyle] = useState<ThemeToolStyle>(THEME.activeTheme.value.toolStyle);
  const [density, setDensity] = useState<ThemeDensity>(THEME.activeTheme.value.density);
  const [thinking, setThinking] = useState<ThemeThinking>(THEME.activeTheme.value.thinking);

  return (
    <Stack gap="6">
      <Stack gap="2">
        <Text as="span" size="label" weight="semibold">
          Theme
        </Text>
        <div class="onboarding-choice-grid">
          {THEME.themeLibrary.value.slice(0, 6).map((entry) => {
            // `baseThemeId`, not `activeThemeId`: picking a density or tool style
            // promotes the theme to a draft, and a draft's id is `draft` - which
            // made the highlight vanish the moment any option beside it was
            // touched.
            const selected = THEME.baseThemeId.value === entry.id;
            return (
              <Button
                key={entry.id}
                variant={selected ? 'secondary' : 'ghost'}
                class="onboarding-choice min-h-[56px]"
                aria-pressed={selected}
                onClick={() => THEME.selectTheme(entry.id)}
              >
                <span class="flex w-full min-w-0 flex-col items-start gap-1 text-left">
                  <Text as="span" size="label" weight="semibold">
                    {entry.name}
                  </Text>
                  {entry.description ? (
                    <Text as="span" size="tiny" tone="muted">
                      {entry.description}
                    </Text>
                  ) : null}
                </span>
              </Button>
            );
          })}
        </div>
      </Stack>

      <Segment
        label="Tool calls"
        groupLabel="Tool call appearance"
        value={toolStyle}
        choices={TOOL_STYLES}
        hint="Rows keep a turn with many calls compact; cards give each call its own block."
        onChange={(next) => {
          setToolStyle(next);
          THEME.setToolStyle(next);
        }}
      />

      <Segment
        label="Density"
        groupLabel="Density"
        value={density}
        choices={DENSITIES}
        onChange={(next) => {
          setDensity(next);
          THEME.setDensity(next);
        }}
      />

      <Segment
        label="Model reasoning"
        groupLabel="Thinking visibility"
        value={thinking}
        choices={THINKING}
        hint="Changes apply immediately and live in this browser, not in the server config."
        onChange={(next) => {
          setThinking(next);
          THEME.setThinking(next);
        }}
      />
    </Stack>
  );
}
