import type { JSX } from 'preact';

export interface WelcomeStepProps {
  /** The directory this server was started in, when the payload named one. */
  workspace: string | null | undefined;
}

/**
 * Step 1: what this thing is, in three sentences, plus the working directory.
 *
 * The last line is the only fact here that comes from the server; the rest is
 * orientation, so a first-time reader knows what the next five steps will ask
 * and why.
 */
export function WelcomeStep({ workspace }: WelcomeStepProps): JSX.Element {
  return (
    <ul class="onboarding-points">
      <li>
        <strong>Bring an endpoint.</strong> Any OpenAI-compatible API works: OpenAI, OpenRouter, a
        local gateway, a self-hosted vLLM.
      </li>
      <li>
        <strong>Point it at a project.</strong> A project is a directory; every session in it is
        confined to that directory.
      </li>
      <li>
        <strong>You stay in charge.</strong> Approvals decide what runs without asking.
      </li>
      {workspace ? (
        <li>
          Working directory: <code>{workspace}</code>
        </li>
      ) : null}
    </ul>
  );
}
