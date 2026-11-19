import type { ComponentChildren, JSX } from 'preact';
import { useState } from 'preact/hooks';
import { Button, Chip, Input, Text } from '../../ui';

export function Field({
  id,
  label,
  hint,
  error,
  wide = false,
  children,
}: {
  id: string;
  label: string;
  hint?: string;
  error?: string;
  wide?: boolean;
  children: ComponentChildren;
}): JSX.Element {
  return (
    <div class={`field${wide ? ' field-wide' : ''}`}>
      <Text as="label" htmlFor={id} class="field-label">
        {label}
      </Text>
      {children}
      {hint ? (
        <Text as="p" class="field-hint" id={`${id}-hint`}>
          {hint}
        </Text>
      ) : null}
      {error ? (
        <p class="field-error" id={`${id}-error`} role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}

export function ListEditor({
  label,
  hint,
  items,
  placeholder,
  disabled,
  onChange,
}: {
  label: string;
  hint: string;
  items: string[];
  placeholder: string;
  disabled: boolean;
  onChange: (items: string[]) => void;
}): JSX.Element {
  const [draft, setDraft] = useState('');
  const add = (): void => {
    const value = draft.trim();
    if (value === '' || items.includes(value)) {
      setDraft('');
      return;
    }
    onChange([...items, value]);
    setDraft('');
  };
  return (
    <div class="list-editor">
      <Text as="span" class="field-label">
        {label}
      </Text>
      {items.length > 0 ? (
        <ul class="chip-list" aria-label={label}>
          {items.map((item) => (
            <li key={item}>
              <Chip
                removeLabel={`Remove ${item}`}
                onRemove={
                  disabled
                    ? undefined
                    : () => onChange(items.filter((candidate) => candidate !== item))
                }
              >
                <code>{item}</code>
              </Chip>
            </li>
          ))}
        </ul>
      ) : (
        <Text as="p" class="field-hint">
          None.
        </Text>
      )}
      <div class="list-editor-add">
        <Input
          type="text"
          class="flex-1 min-w-0"
          value={draft}
          placeholder={placeholder}
          disabled={disabled}
          aria-label={`Add ${label.toLowerCase()}`}
          onInput={(event) => setDraft(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault();
              add();
            }
          }}
        />
        <Button size="sm" disabled={disabled || draft.trim() === ''} onClick={add}>
          Add
        </Button>
      </div>
      <Text as="p" class="field-hint">
        {hint}
      </Text>
    </div>
  );
}
