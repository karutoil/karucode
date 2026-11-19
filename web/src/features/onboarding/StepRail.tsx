import type { JSX } from 'preact';
import { CheckIcon } from '../../components/icons';
import { STEPS } from './steps';

export interface StepRailProps {
  /** Index of the step on screen; everything before it is done. */
  step: number;
}

/**
 * The progress rail: where the reader is and what is left.
 *
 * It is a progress indicator, not navigation — a click cannot jump to a step
 * that has not had its values saved yet — so the items carry no handler and the
 * labels ellipsise rather than stealing the fields' width.
 */
export function StepRail({ step }: StepRailProps): JSX.Element {
  return (
    <ol class="onboarding-rail" aria-label="Setup steps">
      {STEPS.map((entry, index) => (
        <li
          key={entry.id}
          class={
            index < step
              ? 'onboarding-rail-item is-done'
              : index === step
                ? 'onboarding-rail-item is-current'
                : 'onboarding-rail-item'
          }
          aria-current={index === step ? 'step' : undefined}
        >
          <span class="onboarding-rail-dot" aria-hidden="true">
            {index < step ? <CheckIcon size={11} /> : index + 1}
          </span>
          <span class="onboarding-rail-label">{entry.title}</span>
        </li>
      ))}
    </ol>
  );
}
