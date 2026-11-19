import type { JSX } from 'preact';
import { Toggle } from '../../components/Toggle';
import { Input } from '../../ui';
import { LIMIT_FIELDS } from './formModel';
import { Field } from './SharedControls';
import type { FormState } from './types';

export interface LimitsSectionProps {
  form: FormState;
  errors: Record<string, string>;
  disabled: boolean;
  update: (patch: Partial<FormState>) => void;
}

export function LimitsSection({
  form,
  errors,
  disabled,
  update,
}: LimitsSectionProps): JSX.Element {
  return (
    <section class="settings-section" aria-labelledby="settings-limits">
      <div class="settings-section-head">
        <h3 id="settings-limits">Limits</h3>
        <span class="settings-section-note">Resource and context bounds</span>
      </div>
      <div class="field-grid">
        {LIMIT_FIELDS.map((field) => (
          <Field
            key={field.key}
            id={`limit-${field.key}`}
            label={field.label}
            error={errors[`limits.${field.key}`]}
          >
            <Input
              id={`limit-${field.key}`}
              mono
              type="number"
              // From the field's own rule: a browser spinner that
              // refuses 0 would block the documented "no ceiling".
              min={String(field.min)}
              max={String(field.max)}
              step="1"
              value={form.limits[field.key]}
              disabled={disabled}
              invalid={Boolean(errors[`limits.${field.key}`])}
              aria-describedby={
                errors[`limits.${field.key}`] ? `limit-${field.key}-error` : undefined
              }
              onInput={(event) =>
                update({
                  limits: { ...form.limits, [field.key]: event.currentTarget.value },
                })
              }
            />
          </Field>
        ))}
      </div>
      <div class="field field-toggle">
        <Toggle
          label="Run independent tool calls in parallel"
          description="A turn's reads, and its file writes to different paths, run at the same time instead of one after another. Results still land in the transcript in call order. Shell commands, plugin and MCP tools, and anything that has to ask for approval always run in order. Off runs every call one at a time."
          checked={form.parallelExecution}
          disabled={disabled}
          onChange={(checked) => update({ parallelExecution: checked })}
        />
      </div>
    </section>
  );
}
