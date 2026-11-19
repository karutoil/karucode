import type { ComponentChildren } from 'preact';

/** Spacing steps mapped 1:1 to `--spacing-*` / `--space-*` in `tokens.css` & `tailwind.css`. */
export type SpaceStep = '0' | '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' | '10';

/** Text colour roles guaranteed to meet contrast floors (`faint` is intentionally excluded). */
export type TextTone =
  | 'default'
  | 'secondary'
  | 'muted'
  | 'accent'
  | 'success'
  | 'warn'
  | 'danger'
  | 'info';

/** Type scale steps matching `--fs-*` in `tokens.css`. */
export type TextSize =
  | 'micro'
  | 'tiny'
  | 'small'
  | 'label'
  | 'message'
  | 'body'
  | 'lead'
  | 'title'
  | 'heading'
  | 'display'
  | 'hero';

/** Surface ladder rungs matching `--surface*` in `tokens.css` & `tailwind.css`. */
export type SurfaceTone = 'bg' | 'surface' | 'raised' | 'inset' | 'sunken' | 'rail';

/** Semantic status tones used across `Badge`, `Chip`, `Callout`, and `StatusDot`. */
export type FeedbackTone = 'neutral' | 'accent' | 'info' | 'success' | 'warn' | 'danger';

export type StatusDotTone =
  | 'running'
  | 'ok'
  | 'error'
  | 'danger'
  | 'warn'
  | 'approval'
  | 'interrupted'
  | 'cancelled'
  | 'stopped'
  | 'muted'
  | 'idle'
  | 'tool'
  | 'streaming'
  | 'thinking';

export interface BaseUiProps {
  class?: string;
  id?: string;
  testId?: string;
  children?: ComponentChildren;
}
