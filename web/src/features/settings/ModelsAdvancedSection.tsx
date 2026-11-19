import type { JSX } from 'preact';
import { SettingsAdvanced } from '../../components/SettingsAdvanced';
import { Toggle } from '../../components/Toggle';
import { Input } from '../../ui';
import { Field } from './SharedControls';
import type { FormState } from './types';

export interface ModelsAdvancedSectionProps {
  form: FormState;
  errors: Record<string, string>;
  disabled: boolean;
  advancedOpen: boolean;
  onAdvancedChange: (open: boolean) => void;
  update: (patch: Partial<FormState>) => void;
}

export function ModelsAdvancedSection({
  form,
  errors,
  disabled,
  advancedOpen,
  onAdvancedChange,
  update,
}: ModelsAdvancedSectionProps): JSX.Element {
  return (
    <SettingsAdvanced
      id="settings-models-advanced"
      open={advancedOpen}
      onOpenChange={onAdvancedChange}
      note="Sampling, request timeout and image input"
    >
      <div class="field-grid">
        <Field
          id="temperature"
          label="Temperature"
          error={errors.temperature}
          hint={
            form.sendTemperature
              ? '0–2. Lower is more deterministic.'
              : 'Not sent: the endpoint uses the model’s own default.'
          }
        >
          <Input
            id="temperature"
            mono
            type="number"
            step="0.05"
            min="0"
            max="2"
            value={form.temperature}
            disabled={disabled || !form.sendTemperature}
            invalid={Boolean(errors.temperature)}
            aria-describedby={errors.temperature ? 'temperature-error' : 'temperature-hint'}
            onInput={(event) => update({ temperature: event.currentTarget.value })}
          />
        </Field>
        <Toggle
          label="Send a temperature"
          description="Off by default. An invented sampling value can make a reasoning model repeat itself instead of acting; leave it off unless the endpoint asks for one."
          checked={form.sendTemperature}
          disabled={disabled}
          onChange={(checked) => update({ sendTemperature: checked })}
        />
        <Field id="max-tokens" label="Max tokens" error={errors.maxTokens} hint="Empty means the provider default.">
          <Input
            id="max-tokens"
            mono
            type="number"
            min="1"
            step="1"
            value={form.maxTokens}
            disabled={disabled}
            invalid={Boolean(errors.maxTokens)}
            aria-describedby={errors.maxTokens ? 'max-tokens-error' : 'max-tokens-hint'}
            onInput={(event) => update({ maxTokens: event.currentTarget.value })}
          />
        </Field>
        <Field id="top-p" label="Top-p" error={errors.topP} hint="Empty means the provider default.">
          <Input
            id="top-p"
            mono
            type="number"
            min="0"
            max="1"
            step="0.05"
            value={form.topP}
            disabled={disabled}
            invalid={Boolean(errors.topP)}
            aria-describedby={errors.topP ? 'top-p-error' : 'top-p-hint'}
            onInput={(event) => update({ topP: event.currentTarget.value })}
          />
        </Field>
        <Field
          id="request-timeout"
          label="Request timeout (s)"
          error={errors.requestTimeout}
          hint="Per provider request."
        >
          <Input
            id="request-timeout"
            mono
            type="number"
            min="1"
            max="3600"
            step="1"
            value={form.requestTimeout}
            disabled={disabled}
            invalid={Boolean(errors.requestTimeout)}
            aria-describedby={
              errors.requestTimeout ? 'request-timeout-error' : 'request-timeout-hint'
            }
            onInput={(event) => update({ requestTimeout: event.currentTarget.value })}
          />
        </Field>
        <div class="field field-toggle">
          <Toggle
            label="Parallel tool calls"
            description="Ask the endpoint to allow multiple tool calls per turn."
            checked={form.parallelToolCalls}
            disabled={disabled}
            onChange={(checked) => update({ parallelToolCalls: checked })}
          />
        </div>
        <div class="field field-toggle">
          <Toggle
            label="Send images to models"
            description="Install-wide default for read_file on an image. A model's own “Image input (vision)” setting overrides it, so a vision model still works while this is off. models.dev fills that in when you add a model."
            checked={form.supportsImages}
            disabled={disabled}
            onChange={(checked) => update({ supportsImages: checked })}
          />
        </div>
      </div>
    </SettingsAdvanced>
  );
}
