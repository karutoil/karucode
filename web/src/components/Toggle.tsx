import type { JSX } from 'preact';
import { useId } from 'preact/hooks';

export interface ToggleProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
  id?: string;
  description?: string;
  /**
   * Stable hook for an end-to-end driver (the real-browser verifiers identify
   * controls by `data-testid`, because a label is copy and copy changes).
   */
  testId?: string;
}

export function Toggle({
  checked,
  onChange,
  label,
  disabled = false,
  id,
  description,
  testId,
}: ToggleProps): JSX.Element {
  const generated = useId();
  const descriptionId = description ? `toggle-description-${id ?? generated}` : undefined;
  return (
    <div class="toggle-field">
      <button
        id={id}
        type="button"
        role="switch"
        data-testid={testId}
        aria-checked={checked}
        aria-label={label}
        aria-describedby={descriptionId}
        disabled={disabled}
        class={`toggle${checked ? ' is-on' : ''}`}
        onClick={() => onChange(!checked)}
      >
        <span class="toggle-knob" />
      </button>
      <span class="toggle-label">
        {label}
        {description ? (
          <span class="toggle-description" id={descriptionId}>
            {description}
          </span>
        ) : null}
      </span>
    </div>
  );
}