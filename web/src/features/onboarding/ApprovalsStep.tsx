import type { JSX } from 'preact';
import type { ApprovalMode } from '../../api/types';
import { Button, Text } from '../../ui';
import { APPROVAL_CHOICES } from './steps';

export interface ApprovalsStepProps {
  mode: ApprovalMode;
  onChange: (mode: ApprovalMode) => void;
}

/**
 * Step 4: what the agent may do without asking.
 *
 * A radiogroup of cards rather than a `<select>`, because the four modes are a
 * decision with consequences: each one gets the sentence that says what it will
 * actually let through, instead of a word in a dropdown.
 */
export function ApprovalsStep({ mode, onChange }: ApprovalsStepProps): JSX.Element {
  return (
    <div class="flex flex-col gap-3" role="radiogroup" aria-label="Approval mode">
      {APPROVAL_CHOICES.map((choice) => {
        const checked = mode === choice.value;
        return (
          <Button
            key={choice.value}
            role="radio"
            aria-checked={checked}
            variant={checked ? 'secondary' : 'ghost'}
            class="onboarding-choice min-h-[56px]"
            onClick={() => onChange(choice.value)}
          >
            <span class="flex w-full min-w-0 flex-col items-start gap-1 text-left">
              <Text as="span" size="label" weight="semibold">
                {choice.label}
              </Text>
              <Text as="span" size="tiny" tone="muted">
                {choice.description}
              </Text>
            </span>
          </Button>
        );
      })}
    </div>
  );
}
