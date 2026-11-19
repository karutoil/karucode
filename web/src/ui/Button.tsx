import type { ComponentChildren, JSX, MouseEventHandler, Ref } from 'preact';
import type { BaseUiProps } from './types';
import { cn, variants, type VariantProps } from './variants';

export const buttonVariants = variants({
  base: 'inline-flex items-center justify-center gap-3 rounded-sm border text-small whitespace-nowrap transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-50 enabled:active:translate-y-px',
  variants: {
    variant: {
      secondary:
        'border-line-strong bg-raised text-text hover:enabled:border-accent-line hover:enabled:bg-hover',
      primary:
        'border-accent-fill bg-accent-fill text-on-accent font-semibold hover:enabled:border-accent-hi hover:enabled:bg-accent-hi',
      ghost:
        'border-transparent bg-transparent text-muted hover:enabled:bg-raised hover:enabled:text-text',
      danger:
        'border-danger bg-danger text-bg font-semibold hover:enabled:opacity-90',
      'danger-ghost':
        'border-danger/35 bg-transparent text-danger hover:enabled:border-danger hover:enabled:bg-danger-bg',
      stop:
        'border-danger/45 bg-danger-bg/60 text-danger hover:enabled:border-danger hover:enabled:bg-danger-bg',
    },
    size: {
      sm: 'h-[26px] px-4',
      md: 'h-[30px] px-5',
    },
  },
  defaultVariants: {
    variant: 'secondary',
    size: 'md',
  },
});

export const iconButtonVariants = variants({
  base: 'inline-grid place-items-center flex-none rounded-sm border border-transparent bg-transparent text-muted transition-colors cursor-pointer hover:enabled:bg-raised hover:enabled:text-text active:enabled:bg-hover aria-expanded:bg-raised aria-expanded:text-text aria-pressed:text-accent disabled:cursor-not-allowed disabled:opacity-50',
  variants: {
    size: {
      sm: 'w-[26px] h-[26px]',
      md: 'w-[30px] h-[30px]',
    },
  },
  defaultVariants: {
    size: 'md',
  },
});

export const linkButtonVariants = variants({
  base: 'inline-flex items-center gap-2 px-1 rounded-sm border-0 bg-transparent text-small text-left transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-50',
  variants: {
    tone: {
      muted: 'text-muted hover:enabled:text-accent-text',
      accent: 'text-accent-text hover:enabled:underline',
    },
  },
  defaultVariants: {
    tone: 'muted',
  },
});

export type ButtonVariant = NonNullable<VariantProps<typeof buttonVariants>['variant']>;
export type ButtonSize = NonNullable<VariantProps<typeof buttonVariants>['size']>;
export type LinkButtonTone = NonNullable<VariantProps<typeof linkButtonVariants>['tone']>;

export interface ButtonProps extends BaseUiProps, VariantProps<typeof buttonVariants> {
  loading?: boolean;
  icon?: JSX.Element;
  iconRight?: JSX.Element;
  type?: 'button' | 'submit' | 'reset';
  disabled?: boolean;
  onClick?: MouseEventHandler<HTMLButtonElement>;
  title?: string;
  buttonRef?: Ref<HTMLButtonElement>;
  'data-autofocus'?: boolean;
  'aria-label'?: string;
  'aria-expanded'?: boolean;
  'aria-pressed'?: boolean;
}

function SpinnerIcon(): JSX.Element {
  return (
    <svg
      width={14}
      height={14}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
      focusable="false"
      class="animate-spin shrink-0"
    >
      <circle
        cx="12"
        cy="12"
        r="9"
        stroke="currentColor"
        stroke-width="2"
        opacity="0.25"
      />
      <path
        d="M21 12a9 9 0 0 0-9-9"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
      />
    </svg>
  );
}

export function Button({
  variant = 'secondary',
  size = 'md',
  loading = false,
  icon,
  iconRight,
  type = 'button',
  disabled = false,
  onClick,
  title,
  buttonRef,
  'data-autofocus': dataAutofocus,
  'aria-label': ariaLabel,
  'aria-expanded': ariaExpanded,
  'aria-pressed': ariaPressed,
  testId,
  class: className,
  id,
  children,
}: ButtonProps): JSX.Element {
  const isDisabled = Boolean(disabled || loading);
  return (
    <button
      id={id}
      ref={buttonRef}
      type={type === 'submit' ? 'submit' : type === 'reset' ? 'reset' : 'button'}
      class={cn(buttonVariants({ variant, size }), className)}
      data-variant={variant}
      data-size={size}
      data-testid={testId}
      data-autofocus={dataAutofocus}
      disabled={isDisabled}
      aria-busy={loading ? 'true' : undefined}
      aria-label={ariaLabel}
      aria-expanded={ariaExpanded}
      aria-pressed={ariaPressed}
      title={title}
      onClick={isDisabled ? undefined : onClick}
    >
      {loading ? <SpinnerIcon /> : (icon ?? null)}
      {children}
      {iconRight ?? null}
    </button>
  );
}

export interface IconButtonProps
  extends Omit<BaseUiProps, 'children'>,
    VariantProps<typeof iconButtonVariants> {
  label: string;
  icon: JSX.Element;
  pressed?: boolean;
  expanded?: boolean;
  disabled?: boolean;
  type?: 'button' | 'submit' | 'reset';
  title?: string;
  buttonRef?: Ref<HTMLButtonElement>;
  'aria-haspopup'?: JSX.IntrinsicElements['button']['aria-haspopup'];
  'data-autofocus'?: boolean;
  onClick?: MouseEventHandler<HTMLButtonElement>;
}

export function IconButton({
  label,
  icon,
  size = 'md',
  pressed,
  expanded,
  disabled = false,
  type = 'button',
  title,
  buttonRef,
  'aria-haspopup': ariaHasPopup,
  'data-autofocus': dataAutofocus,
  onClick,
  testId,
  class: className,
  id,
}: IconButtonProps): JSX.Element {
  return (
    <button
      id={id}
      ref={buttonRef}
      type={type === 'submit' ? 'submit' : type === 'reset' ? 'reset' : 'button'}
      class={cn(iconButtonVariants({ size }), className)}
      data-size={size}
      data-testid={testId}
      data-autofocus={dataAutofocus}
      aria-label={label}
      aria-pressed={pressed}
      aria-expanded={expanded}
      aria-haspopup={ariaHasPopup}
      disabled={disabled}
      title={title}
      onClick={disabled ? undefined : onClick}
    >
      {icon}
    </button>
  );
}

export interface LinkButtonProps extends BaseUiProps, VariantProps<typeof linkButtonVariants> {
  type?: 'button' | 'submit' | 'reset';
  disabled?: boolean;
  title?: string;
  buttonRef?: Ref<HTMLButtonElement>;
  'aria-label'?: string;
  onClick?: MouseEventHandler<HTMLButtonElement>;
  children?: ComponentChildren;
}

export function LinkButton({
  tone = 'muted',
  type = 'button',
  disabled = false,
  title,
  buttonRef,
  'aria-label': ariaLabel,
  onClick,
  testId,
  class: className,
  id,
  children,
}: LinkButtonProps): JSX.Element {
  return (
    <button
      id={id}
      ref={buttonRef}
      type={type === 'submit' ? 'submit' : type === 'reset' ? 'reset' : 'button'}
      class={cn(linkButtonVariants({ tone }), className)}
      data-tone={tone}
      data-testid={testId}
      aria-label={ariaLabel}
      disabled={disabled}
      title={title}
      onClick={disabled ? undefined : onClick}
    >
      {children}
    </button>
  );
}
