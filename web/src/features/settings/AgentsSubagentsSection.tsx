import type { JSX } from 'preact';
import { Toggle } from '../../components/Toggle';
import { Input } from '../../ui';
import { SUBAGENT_BOUNDS } from './formModel';
import { Field } from './SharedControls';
import type { FormState } from './types';

export interface AgentsSubagentsSectionProps {
  form: FormState;
  errors: Record<string, string>;
  disabled: boolean;
  update: (patch: Partial<FormState>) => void;
}

export function AgentsSubagentsSection({
  form,
  errors,
  disabled,
  update,
}: AgentsSubagentsSectionProps): JSX.Element {
  return (
    <section class="settings-section" aria-labelledby="settings-subagents">
      <div class="settings-section-head">
        <h3 id="settings-subagents">Subagents</h3>
        <span class="settings-section-note">Same workspace, advisory path ownership</span>
      </div>
      <div class="field-grid">
        <Field
          id="subagent-concurrent"
          label="Max concurrent"
          error={errors.subagentConcurrent}
          hint="Loops running at once, install-wide. 0 = no ceiling: a fan-out starts every child it was given."
        >
          <Input
            id="subagent-concurrent"
            mono
            type="number"
            min={SUBAGENT_BOUNDS.concurrent.min}
            max={SUBAGENT_BOUNDS.concurrent.max}
            step="1"
            value={form.subagentConcurrent}
            disabled={disabled}
            invalid={Boolean(errors.subagentConcurrent)}
            aria-describedby={
              errors.subagentConcurrent
                ? 'subagent-concurrent-error'
                : 'subagent-concurrent-hint'
            }
            onInput={(event) => update({ subagentConcurrent: event.currentTarget.value })}
          />
        </Field>
        <Field
          id="subagent-iterations"
          label="Turns per subagent (0 = no ceiling)"
          error={errors.subagentIterations}
          hint="0 by default: a delegated task runs until it is done or its timeout is up."
        >
          <Input
            id="subagent-iterations"
            mono
            type="number"
            min="0"
            max="10000"
            step="1"
            value={form.subagentIterations}
            disabled={disabled}
            invalid={Boolean(errors.subagentIterations)}
            aria-describedby={
              errors.subagentIterations
                ? 'subagent-iterations-error'
                : 'subagent-iterations-hint'
            }
            onInput={(event) => update({ subagentIterations: event.currentTarget.value })}
          />
        </Field>
        <Field
          id="subagent-max-depth"
          label="Max depth"
          error={errors.subagentMaxDepth}
          hint="How many generations may nest. 0 = no delegation at all; 255 is the deepest the field holds."
        >
          <Input
            id="subagent-max-depth"
            mono
            type="number"
            min={SUBAGENT_BOUNDS.maxDepth.min}
            max={SUBAGENT_BOUNDS.maxDepth.max}
            step="1"
            value={form.subagentMaxDepth}
            disabled={disabled}
            invalid={Boolean(errors.subagentMaxDepth)}
            aria-describedby={
              errors.subagentMaxDepth ? 'subagent-max-depth-error' : 'subagent-max-depth-hint'
            }
            onInput={(event) => update({ subagentMaxDepth: event.currentTarget.value })}
          />
        </Field>
        <Field id="subagent-timeout" label="Timeout (s)" error={errors.subagentTimeout}>
          <Input
            id="subagent-timeout"
            mono
            type="number"
            min="1"
            max="604800"
            step="1"
            value={form.subagentTimeout}
            disabled={disabled}
            invalid={Boolean(errors.subagentTimeout)}
            aria-describedby={errors.subagentTimeout ? 'subagent-timeout-error' : undefined}
            onInput={(event) => update({ subagentTimeout: event.currentTarget.value })}
          />
        </Field>
        <Field
          id="subagent-max-background"
          label="Live background agents"
          hint="How many detached subagents one session may keep running at once. 0 = no ceiling."
          error={errors.subagentMaxBackground}
        >
          <Input
            id="subagent-max-background"
            mono
            type="number"
            min={SUBAGENT_BOUNDS.maxBackground.min}
            max={SUBAGENT_BOUNDS.maxBackground.max}
            step="1"
            value={form.subagentMaxBackground}
            disabled={disabled}
            invalid={Boolean(errors.subagentMaxBackground)}
            aria-describedby={
              errors.subagentMaxBackground
                ? 'subagent-max-background-error'
                : 'subagent-max-background-hint'
            }
            onInput={(event) =>
              update({ subagentMaxBackground: event.currentTarget.value })
            }
          />
        </Field>
        <Field
          id="subagent-wait-timeout"
          label="Wait timeout (s)"
          hint="The default bound on wait_for_agents; a wait that runs out reports who is still working."
          error={errors.subagentWaitTimeout}
        >
          <Input
            id="subagent-wait-timeout"
            mono
            type="number"
            min="1"
            max="3600"
            step="1"
            value={form.subagentWaitTimeout}
            disabled={disabled}
            invalid={Boolean(errors.subagentWaitTimeout)}
            aria-describedby={
              errors.subagentWaitTimeout
                ? 'subagent-wait-timeout-error'
                : 'subagent-wait-timeout-hint'
            }
            onInput={(event) =>
              update({ subagentWaitTimeout: event.currentTarget.value })
            }
          />
        </Field>
        <div class="field field-toggle field-wide">
          <Toggle
            label="Allow background subagents"
            description="A spawn can return immediately and keep working; the parent collects it with wait_for_agents."
            checked={form.allowBackground}
            disabled={disabled}
            onChange={(checked) => update({ allowBackground: checked })}
          />
        </div>
        <div class="field field-toggle field-wide">
          <Toggle
            label="Allow recursive subagents"
            description="Subagents may spawn their own subagents, bounded by depth."
            checked={form.allowRecursive}
            disabled={disabled}
            onChange={(checked) => update({ allowRecursive: checked })}
          />
        </div>
        <div class="field field-toggle field-wide">
          <Toggle
            label="Give each subagent its own worktree"
            description="Off by default: subagents share this workspace. On, each child gets a detached git checkout of the current working state and its changes come back on a karucode/subagent/<id> branch. Needs a git repository with at least one commit."
            checked={form.subagentWorktrees}
            disabled={disabled}
            onChange={(checked) => update({ subagentWorktrees: checked })}
          />
        </div>
      </div>
    </section>
  );
}
