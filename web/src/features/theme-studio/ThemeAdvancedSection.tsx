/* Advanced customization: the Custom CSS editor and the variable export.
 *
 * Custom CSS is a trust boundary, and the note beside the editor is where that
 * has to be said: the CSS is injected verbatim into this document (no sanitiser
 * beyond the size cap), and an imported theme file carries its own CSS with it,
 * so the file — not just the colours — is something the user is trusting.
 * Saying "cannot load remote resources" would be a lie: `url()` and `@import`
 * both work.
 */

import type { JSX } from 'preact';
import { useId } from 'preact/hooks';
import { MAX_CUSTOM_CSS, activeTheme, setCustomCSS } from '../../theme';
import { Callout, Inline, LinkButton, Text, Textarea, icons } from '../../ui';

export interface ThemeAdvancedSectionProps {
  copyVariables: () => Promise<void>;
}

export function ThemeAdvancedSection({ copyVariables }: ThemeAdvancedSectionProps): JSX.Element {
  const baseId = useId();
  const headingId = `${baseId}-advanced`;
  const cssId = `${baseId}-css`;
  const helpId = `${baseId}-css-help`;
  const trustId = `${baseId}-css-trust-note`;

  const theme = activeTheme.value;

  return (
    <section class="theme-section" aria-labelledby={headingId}>
      <div class="theme-section-title">
        <Text as="h3" id={headingId} size="label" weight="bold">
          Advanced customization
        </Text>
      </div>
      <Text as="p" class="theme-description" size="tiny" tone="muted">
        Full control: add your own selectors, animations, effects, or overrides. No build tools
        needed.
      </Text>
      <details class="theme-advanced">
        <summary>
          Custom CSS
          <icons.ChevronDownIcon size={14} />
        </summary>
        <div class="theme-advanced-content">
          <label class="sr-only" for={cssId}>
            Custom CSS
          </label>
          <Text as="p" id={helpId} class="theme-advanced-help" size="tiny" tone="muted">
            Applied live after the application stylesheet. Use{' '}
            <code>:root {'{ --theme-accent: #eaa06d !important; }'}</code> to adjust inline theme
            tokens, or target any class.
          </Text>
          <Callout
            tone="warn"
            role="note"
            id={trustId}
            class="theme-trust-note"
            testId="theme-css-trust"
            icon={<icons.AlertIcon size={13} />}
          >
            Only paste CSS you trust. It runs inside this page with the app’s own permissions: it
            can load remote fonts, images and beacons, and it can hide parts of the interface —
            including this drawer. An imported theme carries its CSS with it.
          </Callout>
          <Textarea
            id={cssId}
            class="theme-code-editor"
            spellCheck={false}
            maxLength={MAX_CUSTOM_CSS}
            aria-describedby={`${helpId} ${trustId}`}
            placeholder={
              '/* Add any custom CSS here */\n.project-rail { backdrop-filter: blur(12px); }'
            }
            value={theme.customCSS}
            testId="theme-custom-css"
            onInput={(event) => setCustomCSS(event.currentTarget.value)}
          />
          <Inline gap="5" justify="between" class="theme-advanced-actions">
            <Text as="span" class="theme-code-hint" size="tiny" tone="muted" mono>
              Applied live · saved locally
            </Text>
            <LinkButton tone="accent" class="theme-mini-action" onClick={() => void copyVariables()}>
              <icons.CopyIcon size={13} />
              Copy variables
            </LinkButton>
          </Inline>
        </div>
      </details>
    </section>
  );
}
