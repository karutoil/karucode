import type { JSX } from 'preact';
import { Button, Input, Select, Text } from '../../ui';
import { Field } from './SharedControls';
import type { AddProviderDraft } from './types';

export interface AddProviderFormProps {
  draft: AddProviderDraft;
  draftErrors: Record<string, string>;
  derivedId: string;
  supportedProviders: string[];
  defaultProvider: string;
  disabled: boolean;
  adding: boolean;
  canCancel: boolean;
  onDraftChange: (updater: (current: AddProviderDraft) => AddProviderDraft) => void;
  onCancel: () => void;
  onAdd: () => void;
}

export function AddProviderForm({
  draft,
  draftErrors,
  derivedId,
  supportedProviders,
  defaultProvider,
  disabled,
  adding,
  canCancel,
  onDraftChange,
  onCancel,
  onAdd,
}: AddProviderFormProps): JSX.Element {
  return (
    <div class="provider-add" id="provider-add-form">
      <div class="settings-section-head">
        <Text as="h4" class="provider-add-title">
          New provider
        </Text>
        <Text as="span" class="settings-section-note">
          The key is write-only and never shown again
        </Text>
      </div>
      <div class="field-grid">
        <Field id="new-provider-name" label="Name (optional)" hint="Shown in this list; defaults to the id.">
          <Input
            id="new-provider-name"
            type="text"
            value={draft.name}
            disabled={disabled || adding}
            aria-describedby="new-provider-name-hint"
            onInput={(event) => {
              const value = event.currentTarget.value;
              onDraftChange((current) => ({ ...current, name: value }));
            }}
          />
        </Field>
        <Field
          id="new-provider-id"
          label="Profile id"
          error={draftErrors.id}
          hint={`Used by the API. Leave blank for “${derivedId}”.`}
        >
          <Input
            id="new-provider-id"
            mono
            type="text"
            value={draft.id}
            placeholder={derivedId}
            disabled={disabled || adding}
            invalid={Boolean(draftErrors.id)}
            aria-describedby={draftErrors.id ? 'new-provider-id-error' : 'new-provider-id-hint'}
            onInput={(event) => {
              const value = event.currentTarget.value;
              onDraftChange((current) => ({ ...current, id: value }));
            }}
          />
        </Field>
        <Field id="new-provider-provider" label="API shape" hint="The server implementation that speaks this endpoint.">
          <Select
            id="new-provider-provider"
            class="font-mono"
            value={draft.provider || defaultProvider}
            disabled={disabled || adding}
            aria-describedby="new-provider-provider-hint"
            onChange={(event) => {
              const value = event.currentTarget.value;
              onDraftChange((current) => ({ ...current, provider: value }));
            }}
            options={supportedProviders.map((id) => ({ value: id, label: id }))}
          />
        </Field>
        <Field
          id="new-provider-base-url"
          label="Base URL"
          error={draftErrors.baseUrl}
          hint="Includes the API version, e.g. https://api.example.com/v1."
        >
          <Input
            id="new-provider-base-url"
            mono
            type="url"
            value={draft.baseUrl}
            placeholder="https://api.example.com/v1"
            disabled={disabled || adding}
            invalid={Boolean(draftErrors.baseUrl)}
            aria-describedby={
              draftErrors.baseUrl ? 'new-provider-base-url-error' : 'new-provider-base-url-hint'
            }
            onInput={(event) => {
              const value = event.currentTarget.value;
              onDraftChange((current) => ({ ...current, baseUrl: value }));
            }}
          />
        </Field>
        <Field
          id="new-provider-api-key"
          label="API key"
          wide
          hint="Write-only: stored on the server and never sent back to the browser."
        >
          <Input
            id="new-provider-api-key"
            mono
            type="password"
            placeholder="sk-…"
            value={draft.apiKey}
            disabled={disabled || adding}
            aria-describedby="new-provider-api-key-hint"
            onInput={(event) => {
              const value = event.currentTarget.value;
              onDraftChange((current) => ({ ...current, apiKey: value }));
            }}
          />
        </Field>
      </div>
      <div class="provider-add-actions">
        {canCancel ? (
          <Button size="sm" onClick={onCancel}>
            Cancel
          </Button>
        ) : null}
        <Button
          size="sm"
          variant="primary"
          loading={adding}
          disabled={disabled || adding}
          onClick={onAdd}
        >
          {adding ? 'Adding…' : 'Add provider'}
        </Button>
      </div>
    </div>
  );
}
