import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { ApiError } from '../api/client';
import { sendMessage, sendQueuedMessage } from '../api/endpoints';
import type { PluginPanelRenderProps, PluginPanelRenderer } from './pluginPanels';
import { AlertIcon, CheckIcon } from './icons';

/**
 * The renderer for the `kc-goals` plugin's `goals` panel.
 *
 * ## What this file is, and where it goes
 *
 * A plugin cannot ship UI: the frontend is a compile-time snapshot embedded in
 * the binary. A plugin declares a panel and answers `panels/read` with JSON; the
 * app decides how to draw it. Without this file the tab still works — the
 * generic JSON view renders the payload — but it reads as a data dump. This is
 * the "draw it nicely" half, and it is registered at startup with
 * `registerPluginPanelRenderer`.
 *
 * Copy it to `web/src/components/GoalsPanel.tsx`, the stylesheet to
 * `web/src/styles/goals.css`, and import `goalsPanelRegister` from `main.tsx`
 * (README §8). It is kept next to the plugin because the two are halves of one
 * contract: `panel()` in the plugin's `src/render.rs` produces exactly the
 * payload this file consumes.
 *
 * ## What a button can actually do
 *
 * There is no route that executes a tool. A panel is data; the only way a client
 * can make the agent act is to send it a message. So every action in the payload
 * carries the exact `instruction` to post, and this component posts it as a user
 * turn — `send`, falling back to `follow_up` when the session is already running
 * (the same 409 the plugin's own engine handles). The wording lives in the
 * plugin, not here: if an operation is renamed, this file does not change.
 *
 * ## The security contract
 *
 * `data` is `unknown` and is untrusted: it is whatever JSON the plugin process
 * returned. Everything below validates before reading a field, never reaches
 * `dangerouslySetInnerHTML`, and never builds a URL from it. A goal title is
 * inserted as a text node, so a title containing markup is shown as text.
 */

/** One engine or goal knob, as the plugin serialises it. */
export interface PanelOption {
  key: string;
  label: string;
  kind: string;
  value: string | number | boolean;
  unit?: string;
  /** What a `0` means, e.g. "no cap". Absent when zero is literal. */
  zero_means?: string;
}

/** One offered operation: a label plus the message that performs it. */
export interface PanelAction {
  id: string;
  label: string;
  kind: string;
  enabled: boolean;
  hint: string;
  instruction: string;
}

export interface PanelCheck {
  command: string;
  ok: boolean | null;
  checked_at: number | null;
}

export interface PanelCriterion {
  id: string;
  text: string;
  met: boolean;
  evidence: string | null;
  check: PanelCheck | null;
}

export interface PanelJournalEntry {
  ago: string;
  note: string;
  next: string | null;
}

/** One goal, as the plugin draws it. */
export interface PanelGoal {
  goal: string;
  session_id: string;
  is_current_session: boolean;
  title: string;
  statement: string;
  status: string;
  source: string;
  met: number;
  criteria: number;
  score: string;
  progress_pct: number;
  updated: string;
  continuations: number;
  stalled_resumes: number;
  pause_reason: string | null;
  engine_note: string | null;
  engine: string;
  budget_text: string;
  next: string;
  checks: number;
  checks_pending: number;
  criteria_detail: PanelCriterion[];
  options: PanelOption[];
  actions: PanelAction[];
  journal: PanelJournalEntry[];
}

export interface PanelLogEntry {
  at: string;
  session: string;
  run: string;
  kind: string;
  outcome: string;
}

export interface GoalsPayload {
  schema: string;
  session_id: string;
  workspace: string;
  open: number;
  total: number;
  goals: PanelGoal[];
  log: PanelLogEntry[];
  engine: {
    enabled: boolean;
    auto_continue: boolean;
    prompt: boolean;
    verify_commands: boolean;
    options: PanelOption[];
    actions_note: string;
  };
  /** Set when the plugin itself could not do its job (a bad goal file, no server). */
  error?: string | null;
}

/** The schema this renderer understands. A plugin may add fields freely. */
export const GOALS_SCHEMA_PREFIX = 'kc-goals/panel/';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function str(value: unknown, fallback = ''): string {
  return typeof value === 'string' ? value : fallback;
}

function num(value: unknown, fallback = 0): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

function bool(value: unknown, fallback = false): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

function parseOption(value: unknown): PanelOption | null {
  if (!isRecord(value)) return null;
  const key = str(value.key);
  const label = str(value.label);
  if (!key || !label) return null;
  const raw = value.value;
  if (typeof raw !== 'string' && typeof raw !== 'number' && typeof raw !== 'boolean') return null;
  return {
    key,
    label,
    kind: str(value.kind, 'text'),
    value: raw,
    unit: typeof value.unit === 'string' ? value.unit : undefined,
    zero_means: typeof value.zero_means === 'string' ? value.zero_means : undefined,
  };
}

function parseAction(value: unknown): PanelAction | null {
  if (!isRecord(value)) return null;
  const id = str(value.id);
  const instruction = str(value.instruction);
  const label = str(value.label);
  // An action with no instruction cannot be performed by this component, so it
  // is dropped rather than drawn as a button that does nothing.
  if (!id || !label || !instruction) return null;
  return {
    id,
    label,
    kind: str(value.kind, 'secondary'),
    enabled: bool(value.enabled, true),
    hint: str(value.hint),
    instruction,
  };
}

function parseCriterion(value: unknown): PanelCriterion | null {
  if (!isRecord(value)) return null;
  const id = str(value.id);
  const text = str(value.text);
  if (!id || !text) return null;
  const check = isRecord(value.check)
    ? {
        command: str(value.check.command),
        ok: typeof value.check.ok === 'boolean' ? value.check.ok : null,
        checked_at: typeof value.check.checked_at === 'number' ? value.check.checked_at : null,
      }
    : null;
  return {
    id,
    text,
    met: bool(value.met),
    evidence: typeof value.evidence === 'string' ? value.evidence : null,
    check: check && check.command ? check : null,
  };
}

function parseGoal(value: unknown): PanelGoal | null {
  if (!isRecord(value)) return null;
  const goal = str(value.goal);
  const title = str(value.title);
  if (!goal || !title) return null;
  const criteria_detail = Array.isArray(value.criteria_detail)
    ? value.criteria_detail.map(parseCriterion).filter((c): c is PanelCriterion => c !== null)
    : [];
  const options = Array.isArray(value.options)
    ? value.options.map(parseOption).filter((o): o is PanelOption => o !== null)
    : [];
  const actions = Array.isArray(value.actions)
    ? value.actions.map(parseAction).filter((a): a is PanelAction => a !== null)
    : [];
  const journal = Array.isArray(value.journal)
    ? value.journal
        .filter(isRecord)
        .map((entry) => ({
          ago: str(entry.ago, 'earlier'),
          note: str(entry.note),
          next: typeof entry.next === 'string' ? entry.next : null,
        }))
        .filter((entry) => entry.note)
    : [];
  return {
    goal,
    session_id: str(value.session_id),
    is_current_session: bool(value.is_current_session),
    title,
    statement: str(value.statement),
    status: str(value.status, 'active'),
    source: str(value.source, 'user'),
    met: num(value.met),
    criteria: num(value.criteria),
    score: str(value.score),
    progress_pct: Math.min(100, Math.max(0, num(value.progress_pct))),
    updated: str(value.updated),
    continuations: num(value.continuations),
    stalled_resumes: num(value.stalled_resumes),
    pause_reason: typeof value.pause_reason === 'string' ? value.pause_reason : null,
    engine_note: typeof value.engine_note === 'string' ? value.engine_note : null,
    engine: str(value.engine),
    budget_text: str(value.budget_text),
    next: str(value.next),
    checks: num(value.checks),
    checks_pending: num(value.checks_pending),
    criteria_detail,
    options,
    actions,
    journal,
  };
}

/**
 * Validate a payload, or `undefined` when it is not one of ours.
 *
 * `matches` uses this, so the JSON fallback takes over for an older or foreign
 * payload instead of a half-drawn tab. A payload with a *newer* schema version
 * is accepted: every field is optional to the renderer, and declining it would
 * hide a goal the user asked to see.
 */
export function parseGoalsPayload(data: unknown): GoalsPayload | undefined {
  if (!isRecord(data)) return undefined;
  const schema = str(data.schema);
  if (!schema.startsWith(GOALS_SCHEMA_PREFIX)) return undefined;
  if (!Array.isArray(data.goals)) return undefined;
  const goals = data.goals.map(parseGoal).filter((g): g is PanelGoal => g !== null);
  const engine = isRecord(data.engine) ? data.engine : {};
  const options = Array.isArray(engine.options)
    ? engine.options.map(parseOption).filter((o): o is PanelOption => o !== null)
    : [];
  const log = Array.isArray(data.log)
    ? data.log
        .filter(isRecord)
        .map((entry) => ({
          at: str(entry.at, 'earlier'),
          session: str(entry.session),
          run: str(entry.run),
          kind: str(entry.kind),
          outcome: str(entry.outcome),
        }))
        .filter((entry) => entry.kind)
    : [];
  return {
    schema,
    session_id: str(data.session_id),
    workspace: str(data.workspace),
    open: num(data.open),
    total: num(data.total, goals.length),
    goals,
    log,
    engine: {
      enabled: bool(engine.enabled, true),
      auto_continue: bool(engine.auto_continue, true),
      prompt: bool(engine.prompt, true),
      verify_commands: bool(engine.verify_commands),
      options,
      actions_note: str(engine.actions_note),
    },
    error: typeof data.error === 'string' && data.error ? data.error : null,
  };
}

/** How an option's value reads to a human: `0` may mean "unlimited". */
export function optionText(option: PanelOption): string {
  if (option.value === 0 && option.zero_means) return option.zero_means;
  if (typeof option.value === 'boolean') return option.value ? 'on' : 'off';
  const unit = option.unit ? ` ${option.unit}` : '';
  return `${option.value}${unit}`;
}

type Tone = 'ok' | 'active' | 'waiting' | 'done' | 'bad';

function toneFor(status: string): Tone {
  switch (status) {
    case 'active':
      return 'active';
    case 'paused':
    case 'blocked':
      return 'waiting';
    case 'done':
      return 'done';
    case 'abandoned':
      return 'bad';
    default:
      return 'ok';
  }
}

function checkLabel(check: PanelCheck): { text: string; tone: Tone } {
  if (check.ok === true) return { text: 'passed', tone: 'done' };
  if (check.ok === false) return { text: 'failed', tone: 'bad' };
  return { text: 'not run', tone: 'waiting' };
}

/**
 * Post one action's instruction into the session it belongs to.
 *
 * `409` means a run is in flight; the message is then queued behind it, which
 * is exactly what the composer does. Anything else is shown as the failure it
 * is: a panel must not pretend a message was delivered.
 */
async function sendInstruction(sessionId: string, instruction: string): Promise<void> {
  try {
    await sendMessage(sessionId, instruction);
  } catch (error) {
    if (error instanceof ApiError && error.status === 409) {
      await sendQueuedMessage(sessionId, 'follow_up', { content: instruction });
      return;
    }
    throw error;
  }
}

function StatusPill({ status }: { status: string }): JSX.Element {
  return <span class={`goals-pill goals-pill-${toneFor(status)}`}>{status}</span>;
}

function CriteriaRow({ criterion }: { criterion: PanelCriterion }): JSX.Element {
  const check = criterion.check ? checkLabel(criterion.check) : null;
  return (
    <li class={`goals-criterion${criterion.met ? ' goals-criterion-met' : ''}`}>
      <span class="goals-mark" aria-hidden="true">
        {criterion.met ? <CheckIcon /> : <span class="goals-mark-empty" />}
      </span>
      <div class="goals-criterion-body">
        <div class="goals-criterion-text">
          <span class="goals-criterion-id">{criterion.id}</span> {criterion.text}
        </div>
        {criterion.check && check ? (
          <div class="goals-check">
            <span class={`goals-check-badge goals-pill-${check.tone}`}>{check.text}</span>
            <code class="goals-command">{criterion.check.command}</code>
          </div>
        ) : null}
        {criterion.evidence ? (
          <p class="goals-evidence">{criterion.evidence}</p>
        ) : null}
      </div>
    </li>
  );
}

function OptionGrid({ options }: { options: PanelOption[] }): JSX.Element | null {
  if (options.length === 0) return null;
  return (
    <dl class="goals-options">
      {options.map((option) => (
        <div class="goals-option" key={option.key}>
          <dt>{option.label}</dt>
          <dd>{optionText(option)}</dd>
        </div>
      ))}
    </dl>
  );
}

function GoalCard({
  goal,
  refresh,
}: {
  goal: PanelGoal;
  refresh: () => void;
}): JSX.Element {
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<{ id: string; text: string; bad: boolean } | null>(null);

  async function run(action: PanelAction): Promise<void> {
    if (!action.enabled || busy) return;
    setBusy(action.id);
    setNote(null);
    try {
      await sendInstruction(goal.session_id, action.instruction);
      setNote({ id: action.id, text: 'sent', bad: false });
      refresh();
    } catch (error) {
      const message = error instanceof Error ? error.message : 'could not send';
      setNote({ id: action.id, text: message, bad: true });
    } finally {
      setBusy(null);
    }
  }

  return (
    <section class={`goals-goal goals-goal-${toneFor(goal.status)}`}>
      <header class="goals-goal-head">
        <div class="goals-goal-titles">
          <h3 class="goals-title">{goal.title}</h3>
          <div class="goals-sub">
            <StatusPill status={goal.status} />
            <span class="goals-score">{goal.score} criteria</span>
            <span class="goals-subtle">
              {goal.goal} · {goal.source} · updated {goal.updated}
            </span>
          </div>
        </div>
        <span class="goals-continuations" title="Times the engine resumed this goal">
          {goal.continuations} resumed
        </span>
      </header>

      <div
        class="goals-bar"
        role="progressbar"
        aria-valuenow={goal.progress_pct}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={`${goal.score} criteria met`}
      >
        <div class="goals-bar-fill" style={{ width: `${goal.progress_pct}%` }} />
      </div>

      {goal.statement && goal.statement !== goal.title ? (
        <p class="goals-statement">{goal.statement}</p>
      ) : null}

      {goal.pause_reason ? (
        <p class="goals-note goals-note-waiting">
          <AlertIcon /> {goal.pause_reason}
        </p>
      ) : null}

      {goal.criteria_detail.length > 0 ? (
        <ul class="goals-criteria">
          {goal.criteria_detail.map((criterion) => (
            <CriteriaRow criterion={criterion} key={criterion.id} />
          ))}
        </ul>
      ) : (
        <p class="goals-note">No criteria yet — this goal cannot be completed.</p>
      )}

      <p class="goals-next">Next: {goal.next}</p>

      <div class="goals-engine">
        <span class="goals-engine-text">{goal.engine}</span>
      </div>

      {goal.actions.length > 0 ? (
        <div class="goals-actions">
          {goal.actions.map((action) => (
            <button
              class={`goals-btn goals-btn-${action.kind}`}
              disabled={!action.enabled || busy !== null}
              title={action.hint}
              key={action.id}
              onClick={() => void run(action)}
            >
              {busy === action.id ? '…' : action.label}
            </button>
          ))}
          {note ? (
            <span class={`goals-action-note${note.bad ? ' goals-action-note-bad' : ''}`}>
              {note.text}
            </span>
          ) : null}
        </div>
      ) : null}
      {goal.actions.length > 0 ? (
        <p class="goals-hint">A button sends that instruction to this session as a turn.</p>
      ) : null}

      <details class="goals-details">
        <summary>Limits</summary>
        <OptionGrid options={goal.options} />
        <p class="goals-subtle">{goal.budget_text}</p>
      </details>

      {goal.journal.length > 0 ? (
        <details class="goals-details">
          <summary>Journal ({goal.journal.length})</summary>
          <ul class="goals-journal">
            {goal.journal.map((entry, index) => (
              <li key={index}>
                <span class="goals-subtle">{entry.ago}</span> {entry.note}
                {entry.next ? <em class="goals-journal-next"> → {entry.next}</em> : null}
              </li>
            ))}
          </ul>
        </details>
      ) : null}
    </section>
  );
}

/**
 * The panel itself.
 *
 * Three states, in the order a reader needs them: what went wrong (if
 * anything), the goals, and — folded away — the engine's knobs and the recent
 * decisions, which matter when something did *not* happen.
 */
export function GoalsPanel({ data, refresh }: PluginPanelRenderProps): JSX.Element {
  const parsed = parseGoalsPayload(data);
  if (!parsed) {
    return (
      <p class="goals-note goals-note-bad">
        <AlertIcon /> This panel does not understand its payload (schema{' '}
        {isRecord(data) ? str(data.schema, 'missing') : 'missing'}). The JSON view above still
        shows it.
      </p>
    );
  }

  const engineLine = parsed.engine.enabled
    ? parsed.engine.auto_continue
      ? 'engine on'
      : 'engine idle (auto-continue off)'
    : 'engine off';

  return (
    <div class="goals-tab">
      {/* No refresh button here: `PluginPanelView` already draws one in the
          panel header, and two controls for one fetch is exactly the redundancy
          this removed. `refresh` is still passed down - an action's own message
          can change the goal, so the card rereads the panel after sending it. */}
      <div class="goals-summary">
        <span class="goals-count">
          <strong>{parsed.open}</strong> open
        </span>
        <span class="goals-count">
          <strong>{parsed.total}</strong> total
        </span>
        <span class="goals-count">{engineLine}</span>
        {parsed.engine.verify_commands ? (
          <span class="goals-count goals-count-on">command checks</span>
        ) : null}
      </div>

      {parsed.error ? (
        <p class="goals-note goals-note-bad">
          <AlertIcon /> {parsed.error}
        </p>
      ) : null}

      {parsed.goals.length === 0 ? (
        <p class="goals-note">
          No goals in this workspace. Ask the agent to set one (
          <code>goal_set</code> with a title and at least one criterion), or say what must be
          finished and it will record it.
        </p>
      ) : (
        parsed.goals.map((goal) => (
          <GoalCard
            goal={goal}
            refresh={refresh}
            key={`${goal.session_id}:${goal.goal}`}
          />
        ))
      )}

      <details class="goals-details">
        <summary>Engine options</summary>
        <OptionGrid options={parsed.engine.options} />
        <p class="goals-subtle">{parsed.engine.actions_note}</p>
      </details>

      {parsed.log.length > 0 ? (
        <details class="goals-details">
          <summary>Recent decisions ({parsed.log.length})</summary>
          <ul class="goals-log">
            {parsed.log.map((entry, index) => (
              <li key={index}>
                <span class="goals-subtle">{entry.at}</span>{' '}
                <span class={`goals-log-kind goals-pill-${entry.kind === 'error' ? 'bad' : 'ok'}`}>
                  {entry.kind}
                </span>{' '}
                {entry.outcome}
              </li>
            ))}
          </ul>
        </details>
      ) : null}
    </div>
  );
}

/** The registry entry. `matches` runs before `render`, so it validates the shape. */
export const goalsPanelRenderer: PluginPanelRenderer = {
  matches: (data) => parseGoalsPayload(data) !== undefined,
  render: (props) => <GoalsPanel {...props} />,
};

export const GOALS_PANEL_ID = 'plugin__kc-goals__goals';
