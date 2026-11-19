import type { JSX } from 'preact';
import { useId } from 'preact/hooks';
import { Text } from '../ui/Layout';

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
      {/* The switch stays a plain `<button>`. It is a `switch`, not a pressable
          control: `role="switch"`, `aria-checked` and the `.toggle`/`.toggle-knob`
          track are the contract (`Toggle.test.tsx`, the settings browser
          verifier), and `IconButton` fixes its own role and `aria-pressed`. */}
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
      {/* The two lines of copy are the shared `Text`: `size`/`tone`/`weight` are
          exactly the values `.toggle-label`/`.toggle-description` state
          (`--fs-label` 600, `--fs-tiny` muted 400), so the stack reads the same
          while the type scale lives in one place. */}
      <Text as="span" size="label" weight="semibold" class="toggle-label">
        {label}
        {description ? (
          <Text as="span" size="tiny" tone="muted" weight="normal" id={descriptionId} class="toggle-description">
            {description}
          </Text>
        ) : null}
      </Text>
    </div>
  );
}