import type { JSX } from 'preact';
import { APPROVAL_CHOICES, TUNNELS, type AccessDraft } from './steps';

export interface SummaryStepProps {
  /** The model the server has now; a saved value wins over the draft. */
  model: string;
  baseUrl: string;
  approvalMode: string;
  access: AccessDraft;
  hasCredential: boolean;
}

/**
 * Step 6: what is about to be finished with, in the server's own terms.
 *
 * Every line restates a value the reader chose on an earlier step, so a wrong
 * answer is visible before "Start working" rather than after a run. It is a
 * `<dl>` because that is what it is: five labels and five answers.
 */
export function SummaryStep({
  model,
  baseUrl,
  approvalMode,
  access,
  hasCredential,
}: SummaryStepProps): JSX.Element {
  const approvalLabel =
    APPROVAL_CHOICES.find((choice) => choice.value === approvalMode)?.label ?? approvalMode;
  const signIn = access.enabled
    ? hasCredential
      ? 'Required — a password or passkey is set'
      : 'Required — but no credential is set yet'
    : 'Off (anyone who can reach the port)';
  const tunnel = TUNNELS.find((entry) => entry.value === access.tunnel)?.label;

  return (
    <dl class="onboarding-summary">
      <div>
        <dt>Model</dt>
        <dd>
          {model || '—'}
          {baseUrl ? ` via ${baseUrl}` : ''}
        </dd>
      </div>
      <div>
        <dt>Approvals</dt>
        <dd>{approvalLabel}</dd>
      </div>
      <div>
        <dt>Sign-in</dt>
        <dd>{signIn}</dd>
      </div>
      <div>
        <dt>Address</dt>
        <dd>{access.domain.trim() || 'this browser’s own origin'}</dd>
      </div>
      <div>
        <dt>Tunnel</dt>
        <dd>{tunnel}</dd>
      </div>
    </dl>
  );
}
