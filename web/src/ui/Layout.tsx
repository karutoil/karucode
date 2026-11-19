import type { ComponentChildren, JSX, TargetedSubmitEvent } from 'preact';
import { cn, variants } from './variants';
import type {
  BaseUiProps,
  SpaceStep,
  SurfaceTone,
  TextSize,
  TextTone,
} from './types';

const gapVariants = {
  '0': 'gap-0',
  '1': 'gap-1',
  '2': 'gap-2',
  '3': 'gap-3',
  '4': 'gap-4',
  '5': 'gap-5',
  '6': 'gap-6',
  '7': 'gap-7',
  '8': 'gap-8',
  '9': 'gap-9',
  '10': 'gap-10',
} as const satisfies Record<SpaceStep, string>;

const paddingVariants = {
  '0': 'p-0',
  '1': 'p-1',
  '2': 'p-2',
  '3': 'p-3',
  '4': 'p-4',
  '5': 'p-5',
  '6': 'p-6',
  '7': 'p-7',
  '8': 'p-8',
  '9': 'p-9',
  '10': 'p-10',
} as const satisfies Record<SpaceStep, string>;

const stackVariants = variants({
  base: 'flex flex-col',
  variants: {
    gap: gapVariants,
    align: {
      start: 'items-start',
      center: 'items-center',
      end: 'items-end',
      stretch: 'items-stretch',
    },
    justify: {
      start: 'justify-start',
      center: 'justify-center',
      end: 'justify-end',
      between: 'justify-between',
    },
  },
  defaultVariants: {
    gap: '4',
  },
});

export interface StackProps extends BaseUiProps {
  gap?: SpaceStep;
  align?: 'start' | 'center' | 'end' | 'stretch';
  justify?: 'start' | 'center' | 'end' | 'between';
  as?: 'div' | 'section' | 'form' | 'nav' | 'ul';
  onSubmit?: (event: TargetedSubmitEvent<HTMLFormElement>) => void;
  /**
   * The column's ARIA role, for a stack that is not a plain `<div>`: a `status`
   * for a loading pane, a `group` with a name. Mirrors `Inline`'s prop.
   */
  role?: JSX.IntrinsicElements['div']['role'];
  'aria-label'?: string;
}

export function Stack({
  gap,
  align,
  justify,
  as = 'div',
  onSubmit,
  role,
  'aria-label': ariaLabel,
  id,
  testId,
  class: className,
  children,
}: StackProps): JSX.Element {
  const classes = stackVariants({
    gap,
    align,
    justify,
    class: cn(as === 'ul' && 'list-none', className),
  });

  if (as === 'form') {
    return (
      <form
        id={id}
        class={classes}
        data-testid={testId}
        // A `<form>` accepts only `form`, `none`, `presentation` and `search`,
        // while `role` is typed for the element a stack usually is. The caller
        // chose the pairing; the cast keeps the one prop instead of four.
        role={role as JSX.IntrinsicElements['form']['role']}
        aria-label={ariaLabel}
        onSubmit={onSubmit}
      >
        {children}
      </form>
    );
  }

  // Typed as the `<div>` case on purpose: `section`/`nav`/`ul` take the same
  // props at runtime, but the union of their element types narrows `role` to the
  // intersection of what all four accept, which excludes `status` - the role a
  // loading stack needs. The tag actually rendered is still `as`.
  const Tag = as as 'div';
  return (
    <Tag
      id={id}
      class={classes}
      data-testid={testId}
      role={role}
      aria-label={ariaLabel}
    >
      {children}
    </Tag>
  );
}

const inlineVariants = variants({
  base: 'flex flex-row',
  variants: {
    gap: gapVariants,
    align: {
      start: 'items-start',
      center: 'items-center',
      end: 'items-end',
      baseline: 'items-baseline',
      stretch: 'items-stretch',
    },
    justify: {
      start: 'justify-start',
      center: 'justify-center',
      end: 'justify-end',
      between: 'justify-between',
    },
    wrap: {
      true: 'flex-wrap',
      false: 'flex-nowrap',
    },
  },
  defaultVariants: {
    gap: '3',
    align: 'center',
  },
});

export interface InlineProps extends BaseUiProps {
  gap?: SpaceStep;
  align?: 'start' | 'center' | 'end' | 'baseline' | 'stretch';
  justify?: 'start' | 'center' | 'end' | 'between';
  wrap?: boolean;
  /**
   * The row's ARIA role, for a group that is not a plain `<div>`: a
   * `radiogroup` of segmented options, a `toolbar`, a `group` with a name.
   */
  role?: JSX.IntrinsicElements['div']['role'];
  'aria-label'?: string;
  /** The id of the visible label that names the group. */
  'aria-labelledby'?: string;
  /** The id of the paragraph that explains the group's selection. */
  'aria-describedby'?: string;
}

export function Inline({
  gap,
  align,
  justify,
  wrap,
  role,
  'aria-label': ariaLabel,
  'aria-labelledby': ariaLabelledBy,
  'aria-describedby': ariaDescribedBy,
  id,
  testId,
  class: className,
  children,
}: InlineProps): JSX.Element {
  const classes = inlineVariants({
    gap,
    align,
    justify,
    wrap: wrap === undefined ? undefined : wrap ? 'true' : 'false',
    class: className,
  });

  return (
    <div
      id={id}
      role={role}
      aria-label={ariaLabel}
      aria-labelledby={ariaLabelledBy}
      aria-describedby={ariaDescribedBy}
      class={classes}
      data-testid={testId}
    >
      {children}
    </div>
  );
}

const cardVariants = variants({
  base: 'block',
  variants: {
    surface: {
      bg: 'bg-bg',
      surface: 'bg-surface',
      raised: 'bg-raised',
      inset: 'bg-inset',
      sunken: 'bg-sunken',
      rail: 'bg-rail',
    } satisfies Record<SurfaceTone, string>,
    padding: paddingVariants,
    radius: {
      sm: 'rounded-sm',
      md: 'rounded-md',
      default: 'rounded',
      lg: 'rounded-lg',
    },
    border: {
      soft: 'border border-line-soft',
      default: 'border border-line',
      strong: 'border border-line-strong',
      none: 'border-0',
    },
  },
  defaultVariants: {
    surface: 'surface',
    padding: '6',
    radius: 'default',
    border: 'default',
  },
});

export interface CardProps extends BaseUiProps {
  surface?: SurfaceTone;
  padding?: SpaceStep;
  radius?: 'sm' | 'md' | 'default' | 'lg';
  border?: 'soft' | 'default' | 'strong' | 'none';
  as?: 'div' | 'section' | 'article';
}

export function Card({
  surface,
  padding,
  radius,
  border,
  as = 'div',
  id,
  testId,
  class: className,
  children,
}: CardProps): JSX.Element {
  const classes = cardVariants({
    surface,
    padding,
    radius,
    border,
    class: className,
  });

  const Tag = as;
  return (
    <Tag id={id} class={classes} data-testid={testId}>
      {children}
    </Tag>
  );
}

const textVariants = variants({
  variants: {
    size: {
      micro: 'text-micro',
      tiny: 'text-tiny',
      small: 'text-small',
      label: 'text-label',
      message: 'text-message',
      body: 'text-body',
      lead: 'text-lead',
      title: 'text-title',
      heading: 'text-heading',
      display: 'text-display',
      hero: 'text-hero',
    } satisfies Record<TextSize, string>,
    tone: {
      default: 'text-text',
      secondary: 'text-text-secondary',
      muted: 'text-muted',
      accent: 'text-accent-text',
      success: 'text-success',
      warn: 'text-warn',
      danger: 'text-danger',
      info: 'text-info',
    } satisfies Record<TextTone, string>,
    weight: {
      normal: 'font-normal',
      medium: 'font-medium',
      semibold: 'font-semibold',
      bold: 'font-bold',
    },
    mono: {
      true: 'font-mono',
      false: 'font-ui',
    },
    truncate: {
      true: 'truncate',
    },
  },
  defaultVariants: {
    size: 'body',
    tone: 'default',
  },
});

export interface TextProps extends BaseUiProps {
  as?: 'span' | 'p' | 'div' | 'h1' | 'h2' | 'h3' | 'h4' | 'label' | 'code';
  size?: TextSize;
  tone?: TextTone;
  weight?: 'normal' | 'medium' | 'semibold' | 'bold';
  mono?: boolean;
  truncate?: boolean;
  htmlFor?: string;
  title?: string;
  /** The text's ARIA role, for a line that carries meaning of its own (`status`, `alert`, `note`). */
  role?: JSX.IntrinsicElements['p']['role'];
  'aria-live'?: JSX.IntrinsicElements['div']['aria-live'];
}

export function Text({
  as = 'span',
  size,
  tone,
  weight,
  mono,
  truncate,
  htmlFor,
  title,
  role,
  'aria-live': ariaLive,
  id,
  testId,
  class: className,
  children,
}: TextProps): JSX.Element {
  const classes = textVariants({
    size,
    tone,
    weight,
    mono: mono === undefined ? undefined : mono ? 'true' : 'false',
    truncate: truncate ? 'true' : undefined,
    class: className,
  });

  if (as === 'label') {
    return (
      <label id={id} for={htmlFor} title={title} class={classes} data-testid={testId}>
        {children}
      </label>
    );
  }

  const Tag = as;
  return (
    // `as` is a union of tags, so Preact intersects their `role` types down to
    // the handful every one of them accepts — `status` and `alert` are not in
    // it, even though each tag on its own takes any ARIA role. The cast is that
    // over-narrow intersection, not a real hole: `role` is a valid attribute on
    // every tag `Text` can render.
    <Tag id={id} title={title} role={role as never} aria-live={ariaLive} class={classes} data-testid={testId}>
      {children}
    </Tag>
  );
}

export interface SectionHeaderProps extends Omit<BaseUiProps, 'children'> {
  eyebrow?: string;
  title: string;
  description?: string;
  actions?: ComponentChildren;
}

export function SectionHeader({
  eyebrow,
  title,
  description,
  actions,
  id,
  testId,
  class: className,
}: SectionHeaderProps): JSX.Element {
  const hasActions = actions !== undefined && actions !== null && actions !== false;

  return (
    <div
      id={id}
      class={cn('flex items-start justify-between gap-4', className)}
      data-testid={testId}
    >
      <div class="flex flex-col gap-1 min-w-0 flex-1">
        {eyebrow ? (
          <Text as="span" size="micro" tone="muted" weight="bold" class="uppercase tracking-widest">
            {eyebrow}
          </Text>
        ) : null}
        <Text as="h3" size="heading" tone="default" weight="semibold">
          {title}
        </Text>
        {description ? (
          <Text as="p" size="small" tone="muted" class="max-w-[64ch]">
            {description}
          </Text>
        ) : null}
      </div>
      {hasActions ? <div class="flex items-center gap-2 shrink-0">{actions}</div> : null}
    </div>
  );
}

export interface SettingRowProps extends Omit<BaseUiProps, 'children'> {
  title: string;
  description?: string;
  control: ComponentChildren;
  htmlFor?: string;
  error?: string;
}

export function SettingRow({
  title,
  description,
  control,
  htmlFor,
  error,
  id,
  testId,
  class: className,
}: SettingRowProps): JSX.Element {
  return (
    <div
      id={id}
      class={cn(
        'flex items-center justify-between gap-6 py-4 border-t border-line first:border-t-0',
        className,
      )}
      data-testid={testId}
    >
      <div class="flex flex-col gap-1 min-w-0 flex-1">
        {htmlFor ? (
          <Text as="label" htmlFor={htmlFor} size="label" weight="semibold" tone="default">
            {title}
          </Text>
        ) : (
          <Text as="span" size="label" weight="semibold" tone="default">
            {title}
          </Text>
        )}
        {description ? (
          <Text as="p" size="small" tone="muted">
            {description}
          </Text>
        ) : null}
        {error ? (
          <p role="alert" class="text-tiny text-danger">
            {error}
          </p>
        ) : null}
      </div>
      <div class="shrink-0">{control}</div>
    </div>
  );
}
