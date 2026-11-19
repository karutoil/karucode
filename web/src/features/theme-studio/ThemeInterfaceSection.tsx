/* Interface section: appearance mode, density, both fonts and the corner radius. */

import type { JSX } from 'preact';
import { useId } from 'preact/hooks';
import {
  MAX_ROUNDING,
  MIN_ROUNDING,
  THEME_DENSITIES,
  THEME_FONTS,
  THEME_FONT_LABELS,
  THEME_MONO_FONTS,
  THEME_MONO_LABELS,
  activeTheme,
  setDensity,
  setFont,
  setMono,
  setRounding,
  setThemeMode,
  type ThemeDensity,
  type ThemeFont,
  type ThemeMono,
} from '../../theme';
import { Button, Inline, Input, Select, Text } from '../../ui';
import { DENSITY_LABELS, MODE_LABELS } from './helpers';

export function ThemeInterfaceSection(): JSX.Element {
  const baseId = useId();
  const headingId = `${baseId}-heading`;
  const descriptionId = `${baseId}-desc`;
  const modeLabelId = `${baseId}-mode`;
  const modeHintId = `${baseId}-mode-hint`;
  const densityId = `${baseId}-density`;
  const fontId = `${baseId}-font`;
  const monoId = `${baseId}-mono`;
  const radiusId = `${baseId}-radius`;

  const theme = activeTheme.value;

  return (
    <section class="theme-section" aria-labelledby={headingId}>
      <div class="theme-section-title">
        <Text as="h3" id={headingId} size="label" weight="bold">
          Interface
        </Text>
      </div>
      <Text as="p" class="theme-description" id={descriptionId} size="tiny" tone="muted">
        Control the mood, spacing and typography across the entire workspace.
      </Text>
      <div class="theme-setting-grid">
        <div class="theme-field">
          <Text
            as="span"
            id={modeLabelId}
            class="theme-field-label"
            size="tiny"
            tone="secondary"
            weight="semibold"
          >
            Appearance
          </Text>
          <Inline
            gap="2"
            role="group"
            class="theme-segments"
            aria-labelledby={modeLabelId}
            aria-describedby={modeHintId}
          >
            {(['dark', 'light'] as const).map((mode) => (
              <Button
                key={mode}
                size="sm"
                variant={theme.mode === mode ? 'secondary' : 'ghost'}
                class={theme.mode === mode ? 'theme-segment selected' : 'theme-segment'}
                aria-pressed={theme.mode === mode}
                testId={`theme-mode-${mode}`}
                onClick={() => setThemeMode(mode)}
              >
                {MODE_LABELS[mode]}
              </Button>
            ))}
          </Inline>
          {/* Says what the flip does *before* it happens: the palette is
              re-based to the new mode and the accent survives, which is the
              contract `setThemeMode` and `resetColors` share. */}
          <Text as="p" id={modeHintId} class="theme-field-hint" size="tiny" tone="muted">
            Re-bases the surfaces to that mode and keeps your accent.
          </Text>
        </div>

        <div class="theme-field">
          <Text
            as="label"
            htmlFor={densityId}
            class="theme-field-label"
            size="tiny"
            tone="secondary"
            weight="semibold"
          >
            Density
          </Text>
          <Select
            id={densityId}
            value={theme.density}
            aria-describedby={descriptionId}
            testId="theme-density"
            onChange={(event) => setDensity(event.currentTarget.value as ThemeDensity)}
            options={THEME_DENSITIES.map((density) => ({
              value: density,
              label: DENSITY_LABELS[density],
            }))}
          />
        </div>

        <div class="theme-field">
          <Text
            as="label"
            htmlFor={fontId}
            class="theme-field-label"
            size="tiny"
            tone="secondary"
            weight="semibold"
          >
            Interface font
          </Text>
          <Select
            id={fontId}
            value={theme.font}
            aria-describedby={descriptionId}
            testId="theme-font"
            onChange={(event) => setFont(event.currentTarget.value as ThemeFont)}
            options={THEME_FONTS.map((font) => ({
              value: font,
              label: THEME_FONT_LABELS[font],
            }))}
          />
        </div>

        <div class="theme-field">
          <Text
            as="label"
            htmlFor={monoId}
            class="theme-field-label"
            size="tiny"
            tone="secondary"
            weight="semibold"
          >
            Code font
          </Text>
          <Select
            id={monoId}
            value={theme.mono}
            aria-describedby={descriptionId}
            testId="theme-mono"
            onChange={(event) => setMono(event.currentTarget.value as ThemeMono)}
            options={THEME_MONO_FONTS.map((mono) => ({
              value: mono,
              label: THEME_MONO_LABELS[mono],
            }))}
          />
        </div>
      </div>

      <div class="theme-range-row">
        <Text as="label" htmlFor={radiusId} class="theme-range-label" size="tiny" tone="secondary">
          Corner radius
        </Text>
        <Input
          id={radiusId}
          type="range"
          min={MIN_ROUNDING}
          max={MAX_ROUNDING}
          step={1}
          value={theme.rounding}
          testId="theme-radius"
          onInput={(event) => setRounding(Number(event.currentTarget.value))}
        />
        <output class="theme-range-value" for={radiusId}>
          {theme.rounding}px
        </output>
      </div>
    </section>
  );
}
