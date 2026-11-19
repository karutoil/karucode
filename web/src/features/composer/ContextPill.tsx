import type { JSX } from 'preact';
import { contextTone, formatContextPercent, type ContextUsage } from '../../state/selectors';
import { Button } from '../../ui';

/**
 * The provider-reported `57.4%` context-window reading is also the Usage entry
 * point when the caller has a Usage destination. Without one, the same telemetry
 * stays read-only; the composer never estimates usage from the unsubmitted draft.
 */
export function ContextPill({
  usage,
  onOpenUsage,
}: {
  usage: ContextUsage;
  onOpenUsage?: () => void;
}): JSX.Element {
  const text = formatContextPercent(usage.fraction);
  const tone = contextTone(usage.fraction);
  const title =
    `${usage.tokens.toLocaleString()} of ${usage.window.toLocaleString()} context tokens used (${text}) — ` +
    'the prompt size the provider reported for the newest call. ' +
    'karucode compacts the oldest turns when the request passes this model\'s window. ' +
    'Open Usage for session details.';
  const ariaLabel =
    `Usage: context window ${text} full, ${usage.tokens.toLocaleString()} of ` +
    `${usage.window.toLocaleString()} tokens. Open session usage`;
  const content = (
    <>
      <span class="context-pill-label">Usage</span>
      <span class="context-pill-meter" aria-hidden="true">
        <span class="context-pill-fill" style={{ width: `${Math.max(2, usage.fraction * 100)}%` }} />
      </span>
      <span class="context-pill-text">{text}</span>
    </>
  );

  if (!onOpenUsage) {
    return (
      <span class="context-pill" data-tone={tone} title={title} aria-label={ariaLabel}>
        {content}
      </span>
    );
  }
  return (
    <Button
      variant="ghost"
      size="sm"
      class="context-pill"
      data-tone={tone}
      title={title}
      aria-label={ariaLabel}
      testId="context-pill"
      onClick={() => onOpenUsage()}
    >
      {content}
    </Button>
  );
}
