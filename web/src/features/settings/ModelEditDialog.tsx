import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import type { ModelEntry, ProviderProfile } from '../../api/types';
import { ModelVariantsEditor, type VariantsMap } from '../../components/ModelVariantsEditor';
import { Button, Input, Overlay, Select, Text } from '../../ui';
import {
  MODEL_COST_LIMIT,
  MODEL_NAME_LIMIT,
  MODEL_TOKEN_LIMIT,
  boundedCost,
  boundedInt,
  triStateOf,
  triStateValue,
} from './providerHelpers';
import { Field } from './SharedControls';
import type { TriState } from './types';

export interface ModelEditDialogProps {
  profile: ProviderProfile;
  model: ModelEntry;
  onSave: (entry: ModelEntry) => Promise<boolean>;
  onClose: () => void;
}

/** Edit one model entry: the label plus every prefilled metadata field. */
export function ModelEditDialog({ profile, model, onSave, onClose }: ModelEditDialogProps): JSX.Element {
  const [name, setName] = useState(model.name ?? '');
  const [contextWindow, setContextWindow] = useState(model.context_window?.toString() ?? '');
  const [maxOutput, setMaxOutput] = useState(model.max_output_tokens?.toString() ?? '');
  const [costInput, setCostInput] = useState(model.cost_input?.toString() ?? '');
  const [costOutput, setCostOutput] = useState(model.cost_output?.toString() ?? '');
  const [toolCall, setToolCall] = useState<TriState>(triStateOf(model.tool_call));
  const [reasoning, setReasoning] = useState<TriState>(triStateOf(model.reasoning));
  const [vision, setVision] = useState<TriState>(triStateOf(model.vision));
  // `null` is the server's "not declared" and `{}` is "declared, offers none".
  // They are different answers (`variant_levels()` returns `None` for one and
  // `Some([])` for the other), so this state keeps them apart.
  const [variants, setVariants] = useState<VariantsMap | null>(model.variants ?? null);
  // Reported by the editor: text on screen it could not parse. The map cannot
  // carry that (it only ever holds valid bodies), so it has to travel beside it.
  const [variantsValid, setVariantsValid] = useState(true);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);

  const submit = async (): Promise<void> => {
    const context = boundedInt(contextWindow, 1, MODEL_TOKEN_LIMIT);
    const output = boundedInt(maxOutput, 1, MODEL_TOKEN_LIMIT);
    const inputCost = boundedCost(costInput, MODEL_COST_LIMIT);
    const outputCost = boundedCost(costOutput, MODEL_COST_LIMIT);
    const next: Record<string, string> = {};
    if (name.trim().length > MODEL_NAME_LIMIT) {
      next.name = `Keep the name to ${MODEL_NAME_LIMIT} characters or fewer.`;
    }
    if (context === undefined) {
      next.context = `Whole tokens between 1 and ${MODEL_TOKEN_LIMIT.toLocaleString()}, or empty.`;
    }
    if (output === undefined) {
      next.output = `Whole tokens between 1 and ${MODEL_TOKEN_LIMIT.toLocaleString()}, or empty.`;
    }
    if (inputCost === undefined) next.costInput = 'A non-negative number (USD per million), or empty.';
    if (outputCost === undefined) next.costOutput = 'A non-negative number (USD per million), or empty.';
    // A reasoning level whose body is still not valid JSON blocks the save.
    // Saving anyway would persist `{}` and silently throw away what was typed.
    if (!variantsValid) {
      next.variants = 'One of the reasoning levels has a body that is not a JSON object.';
    }
    setErrors(next);
    if (Object.keys(next).length > 0) return;
    setSaving(true);
    const ok = await onSave({
      id: model.id,
      name: name.trim() === '' ? null : name.trim(),
      context_window: context ?? null,
      max_output_tokens: output ?? null,
      tool_call: triStateValue(toolCall),
      reasoning: triStateValue(reasoning),
      vision: triStateValue(vision),
      cost_input: inputCost ?? null,
      cost_output: outputCost ?? null,
      catalog_provider: model.catalog_provider ?? null,
      // Carried through explicitly. Without this a saved edit would drop a map
      // written in config.toml, silently reverting the model to the bare
      // `reasoning_effort` string - the one thing this dialog must never do.
      variants: variants ?? null,
    });
    setSaving(false);
    if (ok) onClose();
  };

  return (
    <Overlay
      open
      title={`Edit ${model.id}`}
      onClose={onClose}
      // A form with fields: the `standard` measure. `.dialog-model-edit` in
      // `settings.css` keeps the height cap and lays out the hint, the grid and
      // the action row as direct children of the surface, which is what
      // `bodyLayout="contents"` means - the grid is the only scroller.
      role="standard"
      class="dialog-model-edit"
      bodyLayout="contents"
    >
      <Text as="p" class="field-hint">
        Custom values for this model on {profile.name}. Refreshing the metadata later updates the
        catalog fields but leaves this label untouched.
      </Text>
      <div class="field-grid">
        <Field id="model-edit-name" label="Display name" error={errors.name} wide>
          <Input
            id="model-edit-name"
            type="text"
            value={name}
            placeholder={model.id}
            autoFocus
            invalid={Boolean(errors.name)}
            onInput={(event) => setName(event.currentTarget.value)}
          />
        </Field>
        <Field id="model-edit-context" label="Context window" error={errors.context}>
          <Input
            id="model-edit-context"
            mono
            type="text"
            value={contextWindow}
            placeholder="128000"
            invalid={Boolean(errors.context)}
            onInput={(event) => setContextWindow(event.currentTarget.value)}
          />
        </Field>
        <Field id="model-edit-output" label="Max output tokens" error={errors.output}>
          <Input
            id="model-edit-output"
            mono
            type="text"
            value={maxOutput}
            placeholder="16384"
            invalid={Boolean(errors.output)}
            onInput={(event) => setMaxOutput(event.currentTarget.value)}
          />
        </Field>
        <Field id="model-edit-cost-in" label="Input cost / M tokens" error={errors.costInput}>
          <Input
            id="model-edit-cost-in"
            mono
            type="text"
            value={costInput}
            placeholder="0.15"
            invalid={Boolean(errors.costInput)}
            onInput={(event) => setCostInput(event.currentTarget.value)}
          />
        </Field>
        <Field id="model-edit-cost-out" label="Output cost / M tokens" error={errors.costOutput}>
          <Input
            id="model-edit-cost-out"
            mono
            type="text"
            value={costOutput}
            placeholder="0.60"
            invalid={Boolean(errors.costOutput)}
            onInput={(event) => setCostOutput(event.currentTarget.value)}
          />
        </Field>
        <Field id="model-edit-tools" label="Tool calling">
          <Select
            id="model-edit-tools"
            value={toolCall}
            onChange={(event) => setToolCall(event.currentTarget.value as TriState)}
            options={[
              { value: '', label: 'Unknown' },
              { value: 'true', label: 'Supported' },
              { value: 'false', label: 'Not supported' },
            ]}
          />
        </Field>
        <Field id="model-edit-reasoning" label="Reasoning">
          <Select
            id="model-edit-reasoning"
            value={reasoning}
            onChange={(event) => setReasoning(event.currentTarget.value as TriState)}
            options={[
              { value: '', label: 'Unknown' },
              { value: 'true', label: 'Supported' },
              { value: 'false', label: 'Not supported' },
            ]}
          />
        </Field>
        <Field
          id="model-edit-vision"
          label="Image input (vision)"
          hint={
            vision === ''
              ? 'Not declared: the install-wide “Send images to models” default applies.'
              : vision === 'true'
                ? 'read_file attaches a PNG/JPEG/GIF/WebP for this model to look at.'
                : 'read_file refuses images and says the model cannot see them.'
          }
        >
          <Select
            id="model-edit-vision"
            value={vision}
            aria-describedby="model-edit-vision-hint"
            onChange={(event) => setVision(event.currentTarget.value as TriState)}
            options={[
              { value: '', label: 'Unknown (use the default)' },
              { value: 'true', label: 'Accepted' },
              { value: 'false', label: 'Text only' },
            ]}
          />
        </Field>
        <ModelVariantsEditor
          value={variants}
          onChange={setVariants}
          onValidityChange={setVariantsValid}
          error={errors.variants}
          disabled={saving}
        />
      </div>
      <footer class="dialog-actions">
        <Button size="sm" onClick={onClose} disabled={saving}>
          Cancel
        </Button>
        <Button
          size="sm"
          variant="primary"
          loading={saving}
          disabled={saving}
          onClick={() => void submit()}
        >
          {saving ? 'Saving…' : 'Save model'}
        </Button>
      </footer>
    </Overlay>
  );
}
