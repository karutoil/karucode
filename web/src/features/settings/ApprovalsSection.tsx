import type { JSX } from 'preact';
import type { ApprovalMode } from '../../api/types';
import { AlertIcon } from '../../components/icons';
import { Callout, Input, Select } from '../../ui';
import { APPROVAL_MODES } from './formModel';
import { Field, ListEditor } from './SharedControls';
import type { FormState } from './types';

export interface ApprovalsSectionProps {
  form: FormState;
  errors: Record<string, string>;
  disabled: boolean;
  update: (patch: Partial<FormState>) => void;
}

export function ApprovalsSection({
  form,
  errors,
  disabled,
  update,
}: ApprovalsSectionProps): JSX.Element {
  const approvalDescription =
    APPROVAL_MODES.find((mode) => mode.value === form.approvalMode)?.description ?? '';

  return (
    <section class="settings-section" aria-labelledby="settings-approvals">
      <div class="settings-section-head">
        <h3 id="settings-approvals">Approvals</h3>
        <span class="settings-section-note">The only gate for shell commands</span>
      </div>
      <Field id="approval-mode" label="Approval mode" hint={approvalDescription} wide>
        <Select
          id="approval-mode"
          value={form.approvalMode}
          disabled={disabled}
          aria-describedby="approval-mode-hint"
          onChange={(event) => update({ approvalMode: event.currentTarget.value as ApprovalMode })}
          options={APPROVAL_MODES.map((mode) => ({ value: mode.value, label: mode.label }))}
        />
      </Field>
      {form.approvalMode === 'auto' ? (
        <Callout tone="warn" role="status" icon={<AlertIcon size={14} />}>
          Approve All mode removes the approval gate. Every command will run without asking.
        </Callout>
      ) : null}
      <div class="field-grid">
        <div class="field field-wide">
          <ListEditor
            label="Always allow commands"
            hint="Prefix match. For example, `cargo test` also allows `cargo test -p foo`. Keep this list narrow."
            items={form.allowCommands}
            placeholder="cargo test"
            disabled={disabled}
            onChange={(items) => update({ allowCommands: items })}
          />
        </div>
        <div class="field field-wide">
          <ListEditor
            label="Pre-approved write paths"
            hint="Only consulted in ask mode. Paths are workspace-relative."
            items={form.autoApprovePaths}
            placeholder="docs/"
            disabled={disabled}
            onChange={(items) => update({ autoApprovePaths: items })}
          />
        </div>
        <Field
          id="approval-timeout"
          label="Approval timeout (s)"
          error={errors.approvalTimeout}
          hint="A pending approval is denied when this expires."
        >
          <Input
            id="approval-timeout"
            mono
            type="number"
            min="1"
            max="86400"
            step="1"
            value={form.approvalTimeout}
            disabled={disabled}
            invalid={Boolean(errors.approvalTimeout)}
            aria-describedby={
              errors.approvalTimeout ? 'approval-timeout-error' : 'approval-timeout-hint'
            }
            onInput={(event) => update({ approvalTimeout: event.currentTarget.value })}
          />
        </Field>
      </div>
    </section>
  );
}
