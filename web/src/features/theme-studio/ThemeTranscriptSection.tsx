/* Transcript section: how much of the agent's work the conversation shows. */

import type { JSX } from 'preact';
import { useId } from 'preact/hooks';
import {
  THEME_GROUPING_CHOICES,
  THEME_THINKING_CHOICES,
  THEME_TOOL_STYLE_CHOICES,
  THEME_WORKING_CHOICES,
  activeTheme,
  setGrouping,
  setThinking,
  setToolStyle,
  setWorking,
  type ThemeGrouping,
  type ThemeThinking,
  type ThemeToolStyle,
  type ThemeWorking,
} from '../../theme';
import { Stack, Text } from '../../ui';
import { TranscriptChoice, TranscriptPreview } from './TranscriptChoice';

export function ThemeTranscriptSection(): JSX.Element {
  const headingId = useId();
  const theme = activeTheme.value;

  return (
    <section class="theme-section" aria-labelledby={headingId}>
      <div class="theme-section-title">
        <Text as="h3" id={headingId} size="label" weight="bold">
          Transcript
        </Text>
      </div>
      <Text as="p" class="theme-description" size="tiny" tone="muted">
        Decide how much of the agent’s work the conversation shows at once. Long sessions stay
        readable without hiding anything: every folded step opens in a click.
      </Text>

      <TranscriptPreview />

      <div class="theme-setting-grid theme-transcript-grid">
        <TranscriptChoice
          label="Tool calls"
          value={theme.toolStyle}
          choices={THEME_TOOL_STYLE_CHOICES}
          onChange={(next) => setToolStyle(next as ThemeToolStyle)}
          testId="theme-tool-style"
        />
        <TranscriptChoice
          label="Grouping"
          value={theme.grouping}
          choices={THEME_GROUPING_CHOICES}
          onChange={(next) => setGrouping(next as ThemeGrouping)}
          testId="theme-grouping"
        />
      </div>

      <Stack gap="7" class="theme-transcript-stack">
        <TranscriptChoice
          label="Working steps"
          value={theme.working}
          choices={THEME_WORKING_CHOICES}
          onChange={(next) => setWorking(next as ThemeWorking)}
          testId="theme-working"
        />

        <TranscriptChoice
          label="Reasoning"
          value={theme.thinking}
          choices={THEME_THINKING_CHOICES}
          onChange={(next) => setThinking(next as ThemeThinking)}
          testId="theme-thinking"
        />
      </Stack>

      <Text as="p" class="theme-transcript-note" size="tiny" tone="muted">
        The transcript keeps anything you must act on visible whatever you choose here: a running
        call, a pending approval, an error and a notice are never folded.
      </Text>
    </section>
  );
}
