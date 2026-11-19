import type { ComponentChildren, JSX } from 'preact';
import { useId } from 'preact/hooks';
import type { BaseUiProps } from './types';
import { cn, variants, type VariantProps } from './variants';

export const fieldVariants = variants({
  base: 'w-full',
  variants: {
    layout: {
      vertical: 'flex flex-col gap-2',
      row: 'grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-1',
    },
  },
  defaultVariants: {
    layout: 'vertical',
  },
});

type FieldVariantProps = VariantProps<typeof fieldVariants>;

export interface FieldA11yProps {
  id: string;
  describedBy?: string;
  invalid: boolean;
}

export interface FieldProps extends Omit<BaseUiProps, 'id' | 'children'> {
  label: string;
  htmlFor?: string;
  description?: string;
  error?: string;
  required?: boolean;
  layout?: FieldVariantProps['layout'];
  children: ComponentChildren | ((a11y: FieldA11yProps) => ComponentChildren);
}

export function Field({
  label,
  htmlFor,
  description,
  error,
  required = false,
  layout = 'vertical',
  testId,
  class: className,
  children,
}: FieldProps): JSX.Element {
  const generatedId = useId();
  const controlId = htmlFor ?? generatedId;
  const descriptionId = description ? `${controlId}-description` : undefined;
  const errorId = error ? `${controlId}-error` : undefined;
  const describedByParts = [descriptionId, errorId].filter((part): part is string => Boolean(part));
  const describedBy = describedByParts.length > 0 ? describedByParts.join(' ') : undefined;
  const invalid = Boolean(error);

  const content =
    typeof children === 'function' ? children({ id: controlId, describedBy, invalid }) : children;

  const labelEl = (
    <label
      for={controlId}
      class={cn('inline-flex items-center gap-1 text-label font-semibold text-text-secondary')}
    >
      <span>{label}</span>
      {required ? (
        <span aria-hidden="true" class="text-danger">
          *
        </span>
      ) : null}
    </label>
  );

  const descriptionEl = description ? (
    <p id={descriptionId} class="text-tiny text-muted break-words">
      {description}
    </p>
  ) : null;

  const errorEl = error ? (
    <p id={errorId} role="alert" class={cn('text-tiny text-danger', layout === 'row' && 'col-span-2')}>
      {error}
    </p>
  ) : null;

  return (
    <div
      data-testid={testId}
      data-invalid={invalid ? 'true' : undefined}
      class={fieldVariants({ layout, class: className })}
    >
      {layout === 'row' ? (
        <>
          <div class="flex min-w-0 flex-col gap-1">
            {labelEl}
            {descriptionEl}
          </div>
          <div class="shrink-0">{content}</div>
          {errorEl}
        </>
      ) : (
        <>
          {labelEl}
          {content}
          {descriptionEl}
          {errorEl}
        </>
      )}
    </div>
  );
}
