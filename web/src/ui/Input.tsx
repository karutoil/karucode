import type { ComponentChildren, JSX, Ref } from 'preact';
import { useId } from 'preact/hooks';
import type { BaseUiProps } from './types';
import { cn, variants, type VariantProps } from './variants';

export const inputVariants = variants({
  base: 'w-full rounded-sm border border-line bg-rail text-text placeholder:text-muted transition-colors duration-150 enabled:hover:border-line-strong focus-visible:border-accent-line disabled:cursor-not-allowed disabled:opacity-50 read-only:bg-surface',
  variants: {
    size: {
      sm: 'h-[28px] px-3 text-tiny',
      md: 'h-[32px] px-4',
    },
    mono: {
      true: 'font-mono',
      false: '',
    },
    invalid: {
      true: 'border-danger focus-visible:border-danger enabled:hover:border-danger',
      false: '',
    },
  },
  defaultVariants: {
    size: 'md',
    mono: 'false',
    invalid: 'false',
  },
  compoundVariants: [
    {
      variants: { size: 'md', mono: 'false' },
      class: 'text-small',
    },
    {
      variants: { size: 'md', mono: 'true' },
      class: 'text-message',
    },
  ],
});

type InputVariantProps = VariantProps<typeof inputVariants>;

export interface InputProps extends Omit<BaseUiProps, 'children'> {
  type?: string;
  value?: string | number;
  defaultValue?: string | number;
  placeholder?: string;
  mono?: boolean;
  size?: InputVariantProps['size'];
  invalid?: boolean;
  disabled?: boolean;
  readOnly?: boolean;
  required?: boolean;
  min?: number | string;
  max?: number | string;
  step?: number | string;
  /** Upper bound on typed characters, for a field the server also caps. */
  maxLength?: number;
  autoFocus?: boolean;
  autoComplete?: string;
  spellCheck?: boolean;
  name?: string;
  /** A `<datalist id>` offering suggestions without forcing a choice. */
  list?: string;
  inputRef?: Ref<HTMLInputElement>;
  'data-autofocus'?: boolean;
  'data-escape-local'?: boolean;
  'aria-describedby'?: string;
  'aria-label'?: string;
  'aria-controls'?: string;
  onInput?: JSX.IntrinsicElements['input']['onInput'];
  onChange?: JSX.IntrinsicElements['input']['onChange'];
  onKeyDown?: JSX.IntrinsicElements['input']['onKeyDown'];
  onKeyUp?: JSX.IntrinsicElements['input']['onKeyUp'];
  /** IME composition, for a field whose Enter must not submit mid-composition. */
  onCompositionStart?: JSX.IntrinsicElements['input']['onCompositionStart'];
  onCompositionEnd?: JSX.IntrinsicElements['input']['onCompositionEnd'];
  onBlur?: JSX.IntrinsicElements['input']['onBlur'];
  onFocus?: JSX.IntrinsicElements['input']['onFocus'];
}

export function Input({
  type = 'text',
  value,
  defaultValue,
  placeholder,
  mono = false,
  size = 'md',
  invalid = false,
  disabled = false,
  readOnly = false,
  required = false,
  min,
  max,
  step,
  maxLength,
  autoFocus,
  autoComplete,
  spellCheck,
  name,
  list,
  inputRef,
  'data-autofocus': dataAutofocus,
  'data-escape-local': dataEscapeLocal,
  id,
  'aria-describedby': ariaDescribedBy,
  'aria-label': ariaLabel,
  'aria-controls': ariaControls,
  onInput,
  onChange,
  onKeyDown,
  onKeyUp,
  onCompositionStart,
  onCompositionEnd,
  onBlur,
  onFocus,
  testId,
  class: className,
}: InputProps): JSX.Element {
  return (
    <input
      id={id}
      ref={inputRef}
      name={name}
      type={type as 'text'}
      value={value}
      defaultValue={defaultValue}
      placeholder={placeholder}
      disabled={disabled}
      readOnly={readOnly}
      required={required}
      min={min}
      max={max}
      step={step}
      maxLength={maxLength}
      autoFocus={autoFocus}
      autoComplete={autoComplete}
      spellcheck={spellCheck}
      list={list}
      data-autofocus={dataAutofocus}
      data-escape-local={dataEscapeLocal}
      aria-describedby={ariaDescribedBy}
      aria-label={ariaLabel}
      aria-controls={ariaControls}
      aria-invalid={invalid || undefined}
      data-invalid={invalid ? 'true' : undefined}
      data-testid={testId}
      onInput={onInput}
      onChange={onChange}
      onKeyDown={onKeyDown}
      onKeyUp={onKeyUp}
      onCompositionStart={onCompositionStart}
      onCompositionEnd={onCompositionEnd}
      onBlur={onBlur}
      onFocus={onFocus}
      class={inputVariants({
        size,
        mono: mono ? 'true' : 'false',
        invalid: invalid ? 'true' : 'false',
        class: className,
      })}
    />
  );
}

export const textareaVariants = variants({
  base: 'w-full min-h-[72px] rounded-sm border border-line bg-rail px-4 py-2 text-text placeholder:text-muted resize-y transition-colors duration-150 enabled:hover:border-line-strong focus-visible:border-accent-line disabled:cursor-not-allowed disabled:opacity-50 read-only:bg-surface',
  variants: {
    mono: {
      true: 'font-mono text-message',
      false: 'text-small',
    },
    invalid: {
      true: 'border-danger focus-visible:border-danger enabled:hover:border-danger',
      false: '',
    },
  },
  defaultVariants: {
    mono: 'false',
    invalid: 'false',
  },
});

export interface TextareaProps extends Omit<BaseUiProps, 'children'> {
  value?: string;
  defaultValue?: string;
  placeholder?: string;
  rows?: number;
  mono?: boolean;
  invalid?: boolean;
  disabled?: boolean;
  readOnly?: boolean;
  required?: boolean;
  spellCheck?: boolean;
  name?: string;
  autoFocus?: boolean;
  autoComplete?: string;
  textareaRef?: Ref<HTMLTextAreaElement>;
  /** Upper bound on typed characters, for a field the product also caps. */
  maxLength?: number;
  'aria-describedby'?: string;
  'aria-label'?: string;
  onInput?: JSX.IntrinsicElements['textarea']['onInput'];
  onChange?: JSX.IntrinsicElements['textarea']['onChange'];
  onKeyDown?: JSX.IntrinsicElements['textarea']['onKeyDown'];
  onBlur?: JSX.IntrinsicElements['textarea']['onBlur'];
  onFocus?: JSX.IntrinsicElements['textarea']['onFocus'];
}

export function Textarea({
  value,
  defaultValue,
  placeholder,
  rows,
  mono = false,
  invalid = false,
  disabled = false,
  readOnly = false,
  required = false,
  spellCheck,
  autoFocus,
  autoComplete,
  textareaRef,
  maxLength,
  id,
  name,
  'aria-describedby': ariaDescribedBy,
  'aria-label': ariaLabel,
  onInput,
  onChange,
  onKeyDown,
  onBlur,
  onFocus,
  testId,
  class: className,
}: TextareaProps): JSX.Element {
  return (
    <textarea
      id={id}
      ref={textareaRef}
      name={name}
      value={value}
      defaultValue={defaultValue}
      placeholder={placeholder}
      rows={rows}
      maxLength={maxLength}
      disabled={disabled}
      readOnly={readOnly}
      required={required}
      autoFocus={autoFocus}
      autoComplete={autoComplete}
      spellcheck={spellCheck}
      aria-describedby={ariaDescribedBy}
      aria-label={ariaLabel}
      aria-invalid={invalid || undefined}
      data-invalid={invalid ? 'true' : undefined}
      data-testid={testId}
      onInput={onInput}
      onChange={onChange}
      onKeyDown={onKeyDown}
      onBlur={onBlur}
      onFocus={onFocus}
      class={textareaVariants({
        mono: mono ? 'true' : 'false',
        invalid: invalid ? 'true' : 'false',
        class: className,
      })}
    />
  );
}

export const selectVariants = variants({
  base: 'w-full h-[32px] rounded-sm border border-line bg-rail px-4 text-small text-text transition-colors duration-150 enabled:hover:border-line-strong focus-visible:border-accent-line disabled:cursor-not-allowed disabled:opacity-50',
  variants: {
    invalid: {
      true: 'border-danger focus-visible:border-danger enabled:hover:border-danger',
      false: '',
    },
  },
  defaultVariants: {
    invalid: 'false',
  },
});

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

export interface SelectProps extends BaseUiProps {
  value?: string;
  defaultValue?: string;
  options?: readonly SelectOption[];
  invalid?: boolean;
  disabled?: boolean;
  required?: boolean;
  name?: string;
  title?: string;
  'aria-describedby'?: string;
  'aria-label'?: string;
  onInput?: JSX.IntrinsicElements['select']['onInput'];
  onChange?: JSX.IntrinsicElements['select']['onChange'];
  onBlur?: JSX.IntrinsicElements['select']['onBlur'];
  onFocus?: JSX.IntrinsicElements['select']['onFocus'];
  children?: ComponentChildren;
}

export function Select({
  value,
  defaultValue,
  options,
  invalid = false,
  disabled = false,
  required = false,
  id,
  name,
  title,
  'aria-describedby': ariaDescribedBy,
  'aria-label': ariaLabel,
  onInput,
  onChange,
  onBlur,
  onFocus,
  testId,
  class: className,
  children,
}: SelectProps): JSX.Element {
  return (
    <select
      id={id}
      name={name}
      title={title}
      value={value}
      defaultValue={defaultValue}
      disabled={disabled}
      required={required}
      aria-describedby={ariaDescribedBy}
      aria-label={ariaLabel}
      aria-invalid={invalid || undefined}
      data-invalid={invalid ? 'true' : undefined}
      data-testid={testId}
      onInput={onInput}
      onChange={onChange}
      onBlur={onBlur}
      onFocus={onFocus}
      class={selectVariants({
        invalid: invalid ? 'true' : 'false',
        class: className,
      })}
    >
      {options?.map((option) => (
        <option key={option.value} value={option.value} disabled={option.disabled}>
          {option.label}
        </option>
      ))}
      {children}
    </select>
  );
}

export interface CheckboxProps extends Omit<BaseUiProps, 'children'> {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  description?: string;
  disabled?: boolean;
  name?: string;
}

export function Checkbox({
  checked,
  onChange,
  label,
  description,
  disabled = false,
  id,
  name,
  testId,
  class: className,
}: CheckboxProps): JSX.Element {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const descriptionId = description ? `${inputId}-description` : undefined;

  return (
    <label
      for={inputId}
      class={cn(
        'inline-flex items-start gap-2 select-none',
        disabled ? 'cursor-not-allowed opacity-50' : 'cursor-pointer',
        className,
      )}
    >
      <input
        id={inputId}
        name={name}
        type="checkbox"
        checked={checked}
        disabled={disabled}
        aria-describedby={descriptionId}
        data-testid={testId}
        onChange={(event) => onChange(event.currentTarget.checked)}
        class={cn(
          'mt-1 h-[16px] w-[16px] shrink-0 rounded-xs border border-line bg-rail accent-accent-fill',
          disabled ? 'cursor-not-allowed' : 'cursor-pointer',
        )}
      />
      <span class="flex flex-col gap-1">
        <span class="text-small text-text leading-snug">{label}</span>
        {description ? (
          <span id={descriptionId} class="text-tiny text-muted">
            {description}
          </span>
        ) : null}
      </span>
    </label>
  );
}
