import type { JSX } from 'preact';
import { SettingsAdvanced } from '../../components/SettingsAdvanced';
import { AgentsSubagentsSection } from './AgentsSubagentsSection';
import { ApprovalsSection } from './ApprovalsSection';
import { LimitsSection } from './LimitsSection';
import type { FormState } from './types';

export interface AgentSectionProps {
  form: FormState;
  errors: Record<string, string>;
  disabled: boolean;
  advancedOpen: boolean;
  onAdvancedChange: (open: boolean) => void;
  update: (patch: Partial<FormState>) => void;
}

export function AgentSection({
  form,
  errors,
  disabled,
  advancedOpen,
  onAdvancedChange,
  update,
}: AgentSectionProps): JSX.Element {
  return (
    <>
      <ApprovalsSection
        form={form}
        errors={errors}
        disabled={disabled}
        update={update}
      />

      <SettingsAdvanced
        id="settings-agent-advanced"
        open={advancedOpen}
        onOpenChange={onAdvancedChange}
        note="Subagent budget, timeouts and resource limits"
      >
        <AgentsSubagentsSection
          form={form}
          errors={errors}
          disabled={disabled}
          update={update}
        />

        <LimitsSection
          form={form}
          errors={errors}
          disabled={disabled}
          update={update}
        />
      </SettingsAdvanced>
    </>
  );
}
