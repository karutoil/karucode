/* Colour editing: one section per `THEME_COLOR_GROUPS` group.
 *
 * Every cell is a swatch and a hex field over the same token. The swatch's
 * native `input[type=color]` is the one control that stays a raw element: the
 * sheet paints it as an invisible overlay over the swatch (`position: absolute;
 * inset: -6px; opacity: 0`), and `Input`'s field chrome — a fixed 28/32px height
 * and a 1px border — would leave a dead strip under the swatch it is covering.
 */

import type { JSX } from 'preact';
import { useId } from 'preact/hooks';
import {
  THEME_COLOR_GROUPS,
  activeTheme,
  normalizeHex,
  resetColors,
  updateColors,
  type ThemeColorKey,
  type ThemeColorsPatch,
} from '../../theme';
import { Input, LinkButton, Text, icons } from '../../ui';
import { groupCopy } from './helpers';

export interface ThemeColorsSectionProps {
  /** Hex values the user has typed but the engine has not accepted yet. */
  hexEdits: Record<string, string>;
  onHexInput: (key: ThemeColorKey, raw: string) => void;
  onHexBlur: (key: ThemeColorKey) => void;
}

export function ThemeColorsSection({
  hexEdits,
  onHexInput,
  onHexBlur,
}: ThemeColorsSectionProps): JSX.Element {
  const baseId = useId();
  const theme = activeTheme.value;

  return (
    <>
      {THEME_COLOR_GROUPS.map((group) => {
        const copy = groupCopy(group);
        const headingId = `${baseId}-${group.id}`;
        const descriptionId = `${headingId}-desc`;
        return (
          <section class="theme-section" key={group.id} aria-labelledby={headingId}>
            <div class="theme-section-title">
              <Text as="h3" id={headingId} size="label" weight="bold">
                {copy.title}
              </Text>
              {group.id === 'backgrounds' ? (
                <LinkButton tone="accent" class="theme-mini-action" onClick={resetColors}>
                  <icons.RefreshIcon size={12} />
                  Reset colours
                </LinkButton>
              ) : null}
            </div>
            <Text as="p" class="theme-description" id={descriptionId} size="tiny" tone="muted">
              {copy.description}
            </Text>
            <div class="theme-colors">
              {group.fields.map((field) => {
                const edit = hexEdits[field.key];
                const value = theme.colors[field.key];
                const shown = edit ?? value;
                const invalid = edit !== undefined && normalizeHex(edit) === null;
                const inputId = `${baseId}-${field.key}`;
                return (
                  <div class="theme-color-cell" key={field.key}>
                    <span class="theme-color-swatch" style={{ background: value }}>
                      <input
                        id={inputId}
                        type="color"
                        value={value}
                        aria-label={`${field.label} colour picker`}
                        aria-describedby={descriptionId}
                        data-testid={`theme-color-${field.key}`}
                        onInput={(event) =>
                          updateColors({
                            [field.key]: event.currentTarget.value,
                          } as ThemeColorsPatch)
                        }
                      />
                    </span>
                    <span class="theme-color-info">
                      <Text
                        as="label"
                        htmlFor={inputId}
                        class="theme-color-name"
                        size="tiny"
                        tone="secondary"
                        weight="semibold"
                      >
                        {field.label}
                      </Text>
                      <Input
                        class="theme-color-hex"
                        type="text"
                        size="sm"
                        mono
                        value={shown.toUpperCase()}
                        spellCheck={false}
                        maxLength={7}
                        aria-label={`${field.label} hex value`}
                        invalid={invalid}
                        aria-describedby={descriptionId}
                        testId={`theme-hex-${field.key}`}
                        onInput={(event) => onHexInput(field.key, event.currentTarget.value)}
                        onBlur={() => onHexBlur(field.key)}
                      />
                    </span>
                  </div>
                );
              })}
            </div>
          </section>
        );
      })}
    </>
  );
}
