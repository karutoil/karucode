import type { JSX } from 'preact';
import { AlertIcon, CheckIcon } from '../../components/icons';
import { Button, Field, Inline, Input, Select, Stack, Text } from '../../ui';
import type { ProviderTestResult } from './steps';

export interface ProviderStepProps {
  provider: string;
  baseUrl: string;
  apiKey: string;
  model: string;
  /** Whether a key already exists on the server, so blank means "keep it". */
  keyStored: boolean;
  availableProviders: readonly string[];
  testing: boolean;
  testResult: ProviderTestResult | null;
  onProviderChange: (value: string) => void;
  onBaseUrlChange: (value: string) => void;
  onApiKeyChange: (value: string) => void;
  onModelChange: (value: string) => void;
  onTest: () => void;
}

/** The `<datalist>` the "Test connection" answer fills with the endpoint's models. */
const MODEL_SUGGESTIONS_ID = 'onboarding-model-options';

/**
 * Step 2: the endpoint.
 *
 * Only the model id is required. A local gateway (Ollama, vLLM, a llama.cpp
 * server) takes no key, and refusing to continue until one is typed would make
 * the wizard impossible to finish on exactly the install that has no account
 * anywhere.
 */
export function ProviderStep({
  provider,
  baseUrl,
  apiKey,
  model,
  keyStored,
  availableProviders,
  testing,
  testResult,
  onProviderChange,
  onBaseUrlChange,
  onApiKeyChange,
  onModelChange,
  onTest,
}: ProviderStepProps): JSX.Element {
  const providers = availableProviders.length > 0 ? availableProviders : ['openai_compatible'];

  return (
    <Stack gap="4">
      <Field label="Provider" htmlFor="onboarding-provider">
        {(a11y) => (
          <Select
            id={a11y.id}
            value={provider}
            onInput={(event) => onProviderChange(event.currentTarget.value)}
            options={providers.map((id) => ({ value: id, label: id }))}
          />
        )}
      </Field>

      <Field
        label="Base URL"
        htmlFor="onboarding-base-url"
        description="The endpoint root, ending in /v1 for most providers."
      >
        {(a11y) => (
          <Input
            id={a11y.id}
            mono
            type="url"
            spellCheck={false}
            value={baseUrl}
            aria-describedby={a11y.describedBy}
            onInput={(event) => onBaseUrlChange(event.currentTarget.value)}
            placeholder="https://api.openai.com/v1"
          />
        )}
      </Field>

      <Field
        label="API key"
        htmlFor="onboarding-api-key"
        description="Stored on the server, never rendered back. Leave blank for a local endpoint that takes no key."
      >
        {(a11y) => (
          <Input
            id={a11y.id}
            mono
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={apiKey}
            aria-describedby={a11y.describedBy}
            onInput={(event) => onApiKeyChange(event.currentTarget.value)}
            placeholder={keyStored ? 'A key is already stored — leave blank to keep it' : 'sk-…'}
          />
        )}
      </Field>

      <Field
        label="Model"
        htmlFor="onboarding-model"
        description="The model id this endpoint answers to, e.g. gpt-4o-mini."
      >
        {(a11y) => (
          <>
            <Input
              id={a11y.id}
              mono
              spellCheck={false}
              value={model}
              aria-describedby={a11y.describedBy}
              onInput={(event) => onModelChange(event.currentTarget.value)}
              placeholder="gpt-4o-mini"
              list={testResult?.models.length ? MODEL_SUGGESTIONS_ID : undefined}
            />
            {testResult?.models.length ? (
              <datalist id={MODEL_SUGGESTIONS_ID}>
                {testResult.models.map((name) => (
                  <option value={name} key={name} />
                ))}
              </datalist>
            ) : null}
          </>
        )}
      </Field>

      <Inline gap="4" wrap>
        <Button
          size="sm"
          loading={testing}
          disabled={testing || model.trim() === ''}
          onClick={onTest}
        >
          {testing ? 'Testing…' : 'Test connection'}
        </Button>
        {testResult ? (
          <Text
            as="span"
            size="tiny"
            tone={testResult.ok ? 'success' : 'danger'}
            role="status"
            class="onboarding-test"
          >
            {testResult.ok ? <CheckIcon size={13} /> : <AlertIcon size={13} />}
            {testResult.message}
          </Text>
        ) : null}
      </Inline>
    </Stack>
  );
}
