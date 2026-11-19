import type { ComponentChildren, JSX } from 'preact';
import type { BaseUiProps, FeedbackTone, StatusDotTone } from './types';
import { cn, variants } from './variants';

const TONE_CLASSES: Record<FeedbackTone, string> = {
  neutral: 'bg-rail border-line text-muted',
  accent: 'bg-accent-bg border-accent-line text-accent-text',
  info: 'bg-info/10 border-info/35 text-info',
  success: 'bg-success-bg border-success/35 text-success',
  warn: 'bg-warn/10 border-warn/40 text-warn',
  danger: 'bg-danger-bg border-danger/35 text-danger',
};

const badgeVariants = variants({
  base: 'inline-flex items-center gap-1 h-[18px] px-3 border rounded-sm text-micro whitespace-nowrap leading-none',
  variants: {
    tone: TONE_CLASSES,
    mono: {
      true: 'font-mono',
      false: 'font-ui',
    },
  },
  defaultVariants: {
    tone: 'neutral',
    mono: 'true',
  },
});

export interface BadgeProps extends BaseUiProps {
  tone?: FeedbackTone;
  mono?: boolean;
  title?: string;
}

export function Badge({
  tone = 'neutral',
  mono = true,
  title,
  id,
  testId,
  class: className,
  children,
}: BadgeProps): JSX.Element {
  return (
    <span
      id={id}
      title={title}
      data-testid={testId}
      class={badgeVariants({ tone, mono: mono ? 'true' : 'false', class: className })}
    >
      {children}
    </span>
  );
}

const chipVariants = variants({
  base: 'inline-flex items-center gap-2 h-[20px] px-3 border rounded-sm text-tiny whitespace-nowrap',
  variants: {
    tone: TONE_CLASSES,
  },
  defaultVariants: {
    tone: 'neutral',
  },
});

export interface ChipProps extends BaseUiProps {
  tone?: FeedbackTone;
  icon?: JSX.Element;
  onRemove?: () => void;
  removeLabel?: string;
}

export function Chip({
  tone = 'neutral',
  icon,
  onRemove,
  removeLabel,
  id,
  testId,
  class: className,
  children,
}: ChipProps): JSX.Element {
  return (
    <span id={id} data-testid={testId} class={chipVariants({ tone, class: className })}>
      {icon ? (
        <span class="inline-flex items-center shrink-0" aria-hidden="true">
          {icon}
        </span>
      ) : null}
      <span>{children}</span>
      {onRemove ? (
        <button
          type="button"
          class="inline-flex items-center justify-center w-[14px] h-[14px] rounded-xs text-current opacity-75 hover:opacity-100 cursor-pointer"
          aria-label={removeLabel ?? 'Remove'}
          onClick={onRemove}
        >
          <svg
            width={10}
            height={10}
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width={2}
            stroke-linecap="round"
            aria-hidden="true"
            focusable="false"
          >
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      ) : null}
    </span>
  );
}

export interface StatusDotProps extends Omit<BaseUiProps, 'children'> {
  tone: StatusDotTone;
  label?: string;
}

export function StatusDot({
  tone,
  label,
  id,
  testId,
  class: className,
}: StatusDotProps): JSX.Element {
  return (
    <span
      id={id}
      data-testid={testId}
      class={cn('status-dot', `status-${tone}`, className)}
      role={label ? 'img' : undefined}
      aria-label={label || undefined}
      aria-hidden={label ? undefined : 'true'}
    />
  );
}

export type KeycapProps = BaseUiProps;

export function Keycap({ id, testId, class: className, children }: KeycapProps): JSX.Element {
  return (
    <kbd
      id={id}
      data-testid={testId}
      class={cn(
        'inline-flex items-center justify-center bg-raised border border-line-strong rounded-xs font-mono text-micro text-muted px-2 py-1 leading-none whitespace-nowrap',
        className,
      )}
    >
      {children}
    </kbd>
  );
}

export type CalloutTone = 'info' | 'warn' | 'danger' | 'success';

const calloutVariants = variants({
  base: 'flex items-start gap-3 px-4 py-3 border rounded-sm text-small',
  variants: {
    tone: {
      info: 'bg-info/10 border-info/35 text-info',
      warn: 'bg-warn/10 border-warn/40 text-warn',
      danger: 'bg-danger-bg border-danger/35 text-danger',
      success: 'bg-success-bg border-success/35 text-success',
    },
  },
  defaultVariants: {
    tone: 'info',
  },
});

export interface CalloutProps extends BaseUiProps {
  tone?: CalloutTone;
  title?: string;
  icon?: JSX.Element;
  actions?: ComponentChildren;
  /**
   * `status`/`alert` announce themselves; `note` is the one a reader is meant
   * to read in place (a trust boundary beside the control it warns about).
   */
  role?: 'status' | 'alert' | 'note';
}

export function Callout({
  tone = 'info',
  title,
  icon,
  actions,
  role,
  id,
  testId,
  class: className,
  children,
}: CalloutProps): JSX.Element {
  const resolvedRole = role ?? (tone === 'danger' ? 'alert' : 'status');

  return (
    <div
      id={id}
      data-testid={testId}
      role={resolvedRole}
      class={calloutVariants({ tone, class: className })}
    >
      {icon ? (
        <span class="inline-flex items-center shrink-0 mt-1" aria-hidden="true">
          {icon}
        </span>
      ) : null}
      <div class="flex-1 min-w-0 flex flex-col gap-1">
        {title ? <div class="font-semibold">{title}</div> : null}
        {children ? <div class="text-text">{children}</div> : null}
      </div>
      {actions ? <div class="flex items-center gap-2 shrink-0">{actions}</div> : null}
    </div>
  );
}

export interface SpinnerProps extends Omit<BaseUiProps, 'children'> {
  size?: number;
  label?: string;
}

export function Spinner({
  size = 16,
  label,
  id,
  testId,
  class: className,
}: SpinnerProps): JSX.Element {
  return (
    <svg
      id={id}
      data-testid={testId}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width={2}
      stroke-linecap="round"
      role={label ? 'status' : undefined}
      aria-label={label || undefined}
      aria-hidden={label ? undefined : 'true'}
      focusable="false"
      class={cn('inline-block shrink-0 animate-spin', className)}
    >
      <circle cx="12" cy="12" r="9" opacity="0.25" />
      <path d="M21 12a9 9 0 0 0-9-9" />
    </svg>
  );
}

export interface SkeletonProps extends Omit<BaseUiProps, 'children'> {
  label?: string;
}

export function Skeleton({ label, id, testId, class: className }: SkeletonProps): JSX.Element {
  return (
    <div
      id={id}
      data-testid={testId}
      role={label ? 'status' : undefined}
      aria-label={label || undefined}
      aria-busy={label ? 'true' : undefined}
      aria-hidden={label ? undefined : 'true'}
      class={cn('bg-raised border border-line-soft rounded-sm animate-pulse', className)}
    />
  );
}
