import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import type { SkillDetail, SkillListResponse, SkillRoot, SkillSource, SkillSummary } from '../api/types';
import * as skills from '../state/skills';
import { Dialog } from './Dialog';
import { Toggle } from './Toggle';
import { EditIcon, EyeIcon, RefreshIcon, TrashIcon } from './icons';

/**
 * The **Skills** section of the settings sheet: every switch, folder and skill
 * the feature has.
 *
 * It lives here rather than in the results panel because it is install
 * configuration, not a view of one session: the master switch, what the list
 * costs (`prompt_max_chars`, `max_skills`), where discovery looked (the roots, so
 * "auto-detect found nothing" is explainable), and one row per skill with the
 * file it came from. Nothing here is a prompt editor - a skill body is data the
 * agent reads, and the viewer says so.
 *
 * Every control saves itself (`PATCH /api/config` for the switches,
 * `POST`/`PATCH`/`DELETE /api/skills` per skill), so this pane never touches the
 * sheet's form or its Save button - the footer says as much while it is open.
 * It reads once when opened (the sheet keeps panels mounted but the data may be
 * stale after a plugin install) and then only on Refresh or after a mutation;
 * nothing polls.
 */

/**
 * The human label for a source - deliberately *not* the wire value.
 *
 * `data-source` mirrors the API (`workspace`/`user`/`config`/`custom`) so a
 * script can compare the attribute with a response; a person reads the same
 * vocabulary the prompt and the `skill` tool use (`SkillSource::label()`):
 * `configured` for a folder they added, `added` for a skill they pasted in.
 */
const SOURCE_LABEL: Record<SkillSource, string> = {
  workspace: 'workspace',
  user: 'user',
  config: 'configured',
  custom: 'added',
};

function sourceLabel(source: SkillSource): string {
  return SOURCE_LABEL[source];
}

function skillCount(count: number): string {
  return `${count} ${count === 1 ? 'skill' : 'skills'}`;
}

function rootSummary(root: SkillRoot): string {
  // A missing folder and an empty one look identical in a skill list, which is
  // exactly why the roots are shown at all.
  return root.exists ? skillCount(root.count) : 'not found';
}

function SkillRow({
  skill,
  busy,
  onView,
  onEdit,
}: {
  skill: SkillSummary;
  busy: boolean;
  onView: () => void;
  onEdit: () => void;
}): JSX.Element {
  const custom = skill.custom === true;
  return (
    <article
      class="skill-row"
      data-testid="skill-row"
      data-skill-id={skill.id}
      data-enabled={skill.enabled ? 'true' : 'false'}
      data-source={skill.source}
    >
      <div class="skill-row-head">
        <code class="skill-id" title={skill.name === skill.id ? undefined : skill.name}>
          {skill.id}
        </code>
        <span class="skill-source" data-source={skill.source}>
          {sourceLabel(skill.source)}
        </span>
      </div>
      <p class="skill-description">
        {skill.description.trim() === '' ? 'No description — the prompt shows this skill by id only.' : skill.description}
      </p>
      {skill.path ? (
        <p class="skill-path mono" title={skill.path}>
          {skill.path}
        </p>
      ) : (
        <p class="skill-path mono">Stored in config — no file on disk</p>
      )}
      <div class="skill-row-actions">
        <Toggle
          checked={skill.enabled}
          onChange={(next) => void skills.toggleSkill(skill.id, next)}
          label={`Enable ${skill.id}`}
          disabled={busy}
          testId="skill-toggle"
        />
        <button
          type="button"
          class="button button-small"
          data-testid="skill-view"
          aria-label={`View the body of ${skill.id}`}
          disabled={busy}
          onClick={onView}
        >
          <EyeIcon size={12} />
          View
        </button>
        {custom ? (
          <button
            type="button"
            class="button button-small"
            data-testid="skill-edit"
            aria-label={`Edit ${skill.id}`}
            disabled={busy}
            onClick={onEdit}
          >
            <EditIcon size={12} />
            Edit
          </button>
        ) : null}
        {custom ? (
          <button
            type="button"
            class="button button-small button-danger"
            data-testid="skill-delete"
            aria-label={`Delete ${skill.id}`}
            title="Delete this hand-added skill"
            disabled={busy}
            onClick={() => void skills.deleteSkill(skill.id)}
          >
            <TrashIcon size={12} />
            Delete
          </button>
        ) : null}
      </div>
      {custom ? null : (
        <p class="skill-refusal">
          Delete is refused for a skill that came from a file: the row would go and the file would
          stay. Turn it off here, or edit or remove{' '}
          <code class="mono">{skill.path ?? 'its file'}</code>.
        </p>
      )}
    </article>
  );
}

/**
 * The body viewer and the custom-skill editor share one read.
 *
 * `GET /api/skills` carries no bodies, so both modes open a dialog immediately
 * and fill it when `GET /api/skills/:id` answers. The editor is a separate
 * component keyed by id so its draft starts from the body that actually arrived
 * instead of from an empty field that looks like an empty skill.
 */
function SkillDialog({ mode }: { mode: 'view' | 'edit' }): JSX.Element {
  const id = skills.skillBodyId.value;
  const detail = skills.skillBody.value;
  const loading = skills.skillBodyLoading.value;
  const ready = detail !== null && detail.id === id;
  const title = mode === 'edit' ? `Edit skill ${id ?? ''}` : `Skill ${id ?? ''}`;
  return (
    <Dialog
      open={id !== null}
      title={title}
      onClose={skills.closeSkillBody}
      class="dialog-skill"
      description={
        ready && detail.description.trim() !== '' ? detail.description : undefined
      }
    >
      {loading ? <p class="field-hint">Loading {id}…</p> : null}
      {!loading && skills.skillsError.value !== null && detail === null ? (
        <p class="field-error" role="alert">
          {skills.skillsError.value}
        </p>
      ) : null}
      {!loading && ready && detail ? (
        mode === 'edit' ? (
          <SkillEditor detail={detail} />
        ) : (
          <SkillBody detail={detail} />
        )
      ) : null}
    </Dialog>
  );
}

function SkillBody({ detail }: { detail: SkillDetail }): JSX.Element {
  return (
    <div class="skill-body-wrap">
      <p class="field-hint">
        The agent reads this text as data, not as instructions. It reaches the prompt only when the
        task matches the description above.
      </p>
      {detail.body_truncated ? (
        <p class="inline-warning" role="status">
          The server cut this body at its own size cap; the file on disk has the rest.
        </p>
      ) : null}
      <pre class="skill-body" data-testid="skill-body" tabIndex={0}>
        {detail.body ?? ''}
      </pre>
      {detail.path ? (
        <p class="field-hint mono" title={detail.path}>
          {detail.path}
        </p>
      ) : null}
    </div>
  );
}

function SkillEditor({ detail }: { detail: SkillDetail }): JSX.Element {
  const [name, setName] = useState(detail.name);
  const [description, setDescription] = useState(detail.description);
  const [body, setBody] = useState(detail.body ?? '');
  const [problem, setProblem] = useState<string | null>(null);
  const busy = skills.skillsBusy.value;

  const save = async (): Promise<void> => {
    if (body.trim() === '') {
      // The same rule the server applies to `POST`, refused here first so an
      // empty edit cannot blank a skill that took work to write.
      setProblem('A skill needs a body: the agent is told to read one.');
      return;
    }
    setProblem(null);
    const saved = await skills.editSkill(detail.id, { name, description, body });
    if (saved) skills.closeSkillBody();
  };

  return (
    <div class="skill-editor">
      <div class="field">
        <label class="field-label" for="skill-edit-name">
          Name
        </label>
        <input
          id="skill-edit-name"
          class="input"
          type="text"
          value={name}
          data-testid="skill-edit-name"
          onInput={(event) => setName(event.currentTarget.value)}
        />
      </div>
      <div class="field">
        <label class="field-label" for="skill-edit-description">
          Description
        </label>
        <input
          id="skill-edit-description"
          class="input"
          type="text"
          value={description}
          data-testid="skill-edit-description"
          onInput={(event) => setDescription(event.currentTarget.value)}
        />
        <p class="field-hint">One line. This is the only part of a skill the prompt shows.</p>
      </div>
      <div class="field">
        <label class="field-label" for="skill-edit-body">
          Body
        </label>
        <textarea
          id="skill-edit-body"
          class="input skill-editor-body"
          rows={12}
          value={body}
          data-testid="skill-edit-body"
          onInput={(event) => setBody(event.currentTarget.value)}
        />
      </div>
      {problem ? (
        <p class="field-error" role="alert">
          {problem}
        </p>
      ) : null}
      <footer class="dialog-actions">
        <button type="button" class="button" onClick={skills.closeSkillBody} disabled={busy}>
          Cancel
        </button>
        <button
          type="button"
          class="button button-primary"
          data-testid="skill-edit-save"
          disabled={busy}
          onClick={() => void save()}
        >
          {busy ? 'Saving…' : 'Save'}
        </button>
      </footer>
    </div>
  );
}

function AddSkillForm({ disabled }: { disabled: boolean }): JSX.Element {
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [body, setBody] = useState('');
  const [problem, setProblem] = useState<string | null>(null);
  const busy = skills.skillsBusy.value || disabled;

  const add = async (): Promise<void> => {
    if (body.trim() === '') {
      // Refused before any request: the server would answer 400 and the round
      // trip would only make the same point later.
      setProblem('A skill needs a body. Paste the markdown the agent should read.');
      return;
    }
    setProblem(null);
    const created = await skills.addSkill({ name, description, body });
    if (created) {
      setName('');
      setDescription('');
      setBody('');
    }
  };

  return (
    <section class="skills-add" aria-label="Add a skill">
      <div class="field">
        <label class="field-label" for="skill-add-name">
          Name
        </label>
        <input
          id="skill-add-name"
          class="input"
          type="text"
          value={name}
          placeholder="release"
          data-testid="skill-add-name"
          onInput={(event) => setName(event.currentTarget.value)}
        />
        <p class="field-hint">
          Becomes the id (lowercased, slugified). Leave it empty to name the skill after its
          description's first words.
        </p>
      </div>
      <div class="field">
        <label class="field-label" for="skill-add-description">
          Description
        </label>
        <input
          id="skill-add-description"
          class="input"
          type="text"
          value={description}
          placeholder="Cut a release: bump versions, run the gates, tag, push."
          data-testid="skill-add-description"
          onInput={(event) => setDescription(event.currentTarget.value)}
        />
      </div>
      <div class="field">
        <label class="field-label" for="skill-add-body">
          Body
        </label>
        <textarea
          id="skill-add-body"
          class="input skill-editor-body"
          rows={8}
          value={body}
          placeholder={'# Cutting a release\n\n1. `cargo test --workspace` …'}
          data-testid="skill-add-body"
          onInput={(event) => setBody(event.currentTarget.value)}
        />
        <p class="field-hint">Markdown. Required — a skill with no body teaches the agent nothing.</p>
      </div>
      {problem ? (
        <p class="field-error" role="alert" data-testid="skill-add-error">
          {problem}
        </p>
      ) : null}
      <button
        type="button"
        class="button button-primary"
        data-testid="skill-add-submit"
        disabled={busy}
        onClick={() => void add()}
      >
        {busy ? 'Adding…' : 'Add skill'}
      </button>
    </section>
  );
}

/**
 * The two switches that decide how skills behave, each saved the moment it is
 * flipped: a switch that waits for a Save button nowhere near it is a switch
 * people believe they already used.
 */
function MasterSwitch({ data, disabled }: { data: SkillListResponse; disabled: boolean }): JSX.Element {
  const busy = skills.skillsBusy.value || disabled;
  return (
    <Toggle
      checked={data.enabled}
      onChange={(next) => void skills.saveOptions({ enabled: next })}
      label="Skills enabled"
      description="The master switch: off means no skill text in the prompt and the skill tool refuses."
      disabled={busy}
      testId="skills-master-toggle"
    />
  );
}

/** Auto-detection: the conventional folders, on or off. */
function DiscoveryToggle({ data, disabled }: { data: SkillListResponse; disabled: boolean }): JSX.Element {
  const busy = skills.skillsBusy.value || disabled;
  return (
    <Toggle
      checked={data.auto_detect}
      onChange={(next) => void skills.saveOptions({ auto_detect: next })}
      label="Auto-detect skill folders"
      description="Scan this workspace and your home folder for SKILL.md files."
      disabled={busy}
      testId="skills-autodetect-toggle"
    />
  );
}

/**
 * Whether the listing is advertised, and what it may cost.
 *
 * The two numbers are text inputs on purpose: a `type=number` box that silently
 * drops a half-typed value cannot tell a person why nothing was saved. The server
 * clamps both, so the fields are re-seeded from the response and never from what
 * was typed.
 */
function PromptOptions({ data, disabled }: { data: SkillListResponse; disabled: boolean }): JSX.Element {
  const [prompt, setPrompt] = useState(data.prompt);
  const [promptMax, setPromptMax] = useState(String(data.prompt_max_chars));
  const [maxSkills, setMaxSkills] = useState(String(data.max_skills));
  const [problem, setProblem] = useState<string | null>(null);
  const busy = skills.skillsBusy.value || disabled;

  // Re-seed from the stored values, not the whole payload: toggling one skill
  // replaces the data object, and that must not throw away a half-typed number.
  useEffect(() => {
    setPrompt(data.prompt);
    setPromptMax(String(data.prompt_max_chars));
    setMaxSkills(String(data.max_skills));
  }, [data.prompt, data.prompt_max_chars, data.max_skills]);

  const save = async (): Promise<void> => {
    const chars = Number(promptMax.trim());
    if (!Number.isInteger(chars) || chars < 0) {
      setProblem('Prompt budget must be a whole number of characters, 0 or more.');
      return;
    }
    const limit = Number(maxSkills.trim());
    if (!Number.isInteger(limit) || limit < 1) {
      // `1` is the server's own floor (`SkillsConfig::normalize`), so asking for
      // 0 would only be quietly changed back.
      setProblem('Max skills must be a whole number, 1 or more.');
      return;
    }
    setProblem(null);
    await skills.saveOptions({ prompt, prompt_max_chars: chars, max_skills: limit });
  };

  return (
    <>
      <Toggle
        checked={prompt}
        onChange={setPrompt}
        label="List skills in the system prompt"
        description="One line per enabled skill. Off keeps the skill tool, but the agent only finds a skill you name."
        disabled={busy}
        testId="skills-prompt-toggle"
      />
      <div class="field-grid">
        <div class="field">
          <label class="field-label" for="skills-prompt-max">
            Prompt budget (characters)
          </label>
          <input
            id="skills-prompt-max"
            class="input mono"
            type="number"
            min="200"
            max="64000"
            step="100"
            value={promptMax}
            data-testid="skills-prompt-max"
            disabled={busy}
            onInput={(event) => setPromptMax(event.currentTarget.value)}
          />
          <p class="field-hint">
            The cap on skill text in the prompt; the server clamps it to 200–64,000. A long list is
            re-sent and re-billed on every turn.
          </p>
        </div>
        <div class="field">
          <label class="field-label" for="skills-max-skills">
            Max skills
          </label>
          <input
            id="skills-max-skills"
            class="input mono"
            type="number"
            min="1"
            step="1"
            value={maxSkills}
            data-testid="skills-max-skills"
            disabled={busy}
            onInput={(event) => setMaxSkills(event.currentTarget.value)}
          />
          <p class="field-hint">
            How many skills the prompt block lists; the server keeps at least 1. Past the cap, the
            block says so.
          </p>
        </div>
      </div>
      {problem ? (
        <p class="field-error" role="alert">
          {problem}
        </p>
      ) : null}
      <div class="skills-options-actions">
        <button
          type="button"
          class="button button-primary button-small"
          data-testid="skills-save-options"
          disabled={busy}
          onClick={() => void save()}
        >
          {busy ? 'Saving…' : 'Save prompt settings'}
        </button>
        <span class="field-hint">
          Stored with <code>PATCH /api/config</code>; the fields show what was stored, not what was
          typed.
        </span>
      </div>
    </>
  );
}

/** The folders that may be scanned, added and removed one at a time. */
function DirectoriesBlock({ data, disabled }: { data: SkillListResponse; disabled: boolean }): JSX.Element {
  const [draft, setDraft] = useState('');
  const [problem, setProblem] = useState<string | null>(null);
  const busy = skills.skillsBusy.value || disabled;
  const directories = data.directories;

  const add = async (): Promise<void> => {
    const value = draft.trim();
    if (value === '') return;
    if (directories.includes(value)) {
      setProblem(`${value} is already in the list.`);
      return;
    }
    setProblem(null);
    const saved = await skills.saveOptions({ directories: [...directories, value] });
    if (saved) setDraft('');
  };

  return (
    <section class="skills-directories" aria-label="Extra skill folders">
      <span class="field-label">Extra folders</span>
      {directories.length === 0 ? (
        <p class="field-hint">None. Only the detected folders above are scanned.</p>
      ) : (
        <ul class="skills-dir-list">
          {directories.map((directory) => (
            <li class="skills-dir-row" key={directory} data-testid="skills-dir-row" data-dir={directory}>
              <code class="mono" title={directory}>
                {directory}
              </code>
              <button
                type="button"
                class="chip-remove"
                data-testid="skills-dir-remove"
                aria-label={`Remove ${directory}`}
                disabled={busy}
                onClick={() =>
                  void skills.saveOptions({
                    directories: directories.filter((entry) => entry !== directory),
                  })
                }
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
      <div class="list-editor-add">
        <input
          type="text"
          class="input mono"
          value={draft}
          placeholder="/home/me/my-skills or ~/skills"
          aria-label="Add an extra skill folder"
          data-testid="skills-dir-input"
          disabled={busy}
          onInput={(event) => setDraft(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key !== 'Enter') return;
            event.preventDefault();
            void add();
          }}
        />
        <button
          type="button"
          class="button button-small"
          data-testid="skills-dir-add"
          disabled={busy || draft.trim() === ''}
          onClick={() => void add()}
        >
          Add
        </button>
      </div>
      {problem ? (
        <p class="field-error" role="alert">
          {problem}
        </p>
      ) : null}
      <p class="field-hint">
        Absolute paths, or <code>~</code> for your home folder. A folder that does not exist is not
        an error; the roots below say which ones were found.
      </p>
    </section>
  );
}

function RootsBlock({ roots }: { roots: SkillRoot[] }): JSX.Element {
  return (
    <section class="skills-roots" aria-label="Folders discovery looked in">
      <span class="field-label">Roots</span>
      {roots.length === 0 ? (
        <p class="field-hint">
          No folder is being scanned: auto-detect is off and no extra folder is configured.
        </p>
      ) : (
        <ul class="skills-root-list">
          {roots.map((root) => (
            <li
              class="skills-root"
              key={`${root.source}:${root.path}`}
              data-testid="skills-root"
              data-exists={root.exists ? 'true' : 'false'}
              data-source={root.source}
            >
              <code class="skills-root-path" title={root.path}>
                {root.path}
              </code>
              <span class="skills-root-meta">
                <span class="skill-source" data-source={root.source}>
                  {sourceLabel(root.source)}
                </span>
                <span class="skills-root-count" data-exists={root.exists ? 'true' : 'false'}>
                  {rootSummary(root)}
                </span>
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

export function SkillsSettings({ disabled = false }: { disabled?: boolean }): JSX.Element {
  const data = skills.skillsData.value;
  const loading = skills.skillsLoading.value;
  const error = skills.skillsError.value;
  const busy = skills.skillsBusy.value || disabled;
  const staleRoots = skills.skillRootsStale.value;
  const [dialog, setDialog] = useState<'view' | 'edit' | null>(null);

  // One read when the pane is first opened. The sheet keeps panels mounted, so
  // this is "on open" for the sheet's lifetime, not "on every tab switch".
  useEffect(() => {
    void skills.loadSkills();
  }, []);

  // The dialog follows the signal it was opened with, and closes itself if the
  // body read failed and there is nothing to show.
  useEffect(() => {
    if (dialog !== null && skills.skillBodyId.value === null) setDialog(null);
  }, [dialog, skills.skillBodyId.value]);

  const open = (id: string, mode: 'view' | 'edit'): void => {
    setDialog(mode);
    void skills.openSkillBody(id);
  };

  const rows = data?.skills ?? [];
  const total = rows.length;
  const on = rows.filter((skill) => skill.enabled).length;
  const workspaceRoot = data?.roots.find((root) => root.source === 'workspace') ?? data?.roots[0];

  return (
    <div class="skills-settings" data-testid="skills-panel">
      {error !== null ? (
        <p class="field-error skills-error" role="alert" data-testid="skills-error">
          {error}
        </p>
      ) : null}

      <section class="settings-section" aria-labelledby="settings-skills-switch">
        <div class="settings-section-head">
          <h3 id="settings-skills-switch">Skills</h3>
          <span class="settings-section-note">Saved as you change it</span>
        </div>
        {data === null ? (
          <p class="field-hint" role="status">
            {loading ? 'Loading skills…' : 'This server did not answer GET /api/skills; settings that need it are hidden.'}
          </p>
        ) : (
          <>
            <MasterSwitch data={data} disabled={disabled} />
            {!data.enabled ? (
              <div class="skills-master-off" role="status" data-testid="skills-master-off">
                <strong>Skills are off.</strong> No skill text reaches the system prompt and the{' '}
                <code>skill</code> tool refuses. The list below still shows what is installed;
                turning a single skill on or off changes nothing until the master switch is back on.
              </div>
            ) : null}
          </>
        )}
      </section>

      {data === null ? null : (
        <>
          <section class="settings-section" aria-labelledby="settings-skills-discovery">
            <div class="settings-section-head">
              <h3 id="settings-skills-discovery">Where they are found</h3>
              <span class="settings-section-note">Conventional folders, plus your own</span>
            </div>
            <DiscoveryToggle data={data} disabled={disabled} />
            <DirectoriesBlock data={data} disabled={disabled} />
            {staleRoots ? (
              <p class="inline-warning" role="status" data-testid="skills-roots-stale">
                The folders changed. Refresh to see which ones exist and how many skills each holds.
              </p>
            ) : null}
            <RootsBlock roots={data.roots} />
          </section>

          <section class="settings-section" aria-labelledby="settings-skills-prompt">
            <div class="settings-section-head">
              <h3 id="settings-skills-prompt">What the agent sees</h3>
              <span class="settings-section-note">One line each; the body waits for the task</span>
            </div>
            <PromptOptions data={data} disabled={disabled} />
          </section>

          <section class="settings-section" aria-labelledby="settings-skills-installed">
            <div class="settings-section-head">
              <h3 id="settings-skills-installed">Installed</h3>
              <span class="settings-section-note skills-list-head">
                <span data-testid="skills-count">
                  {data === null
                    ? 'loading…'
                    : `${skillCount(total)}${on < total ? ` · ${total - on} off` : ''}`}
                </span>
                <button
                  type="button"
                  class="icon-button inspector-mini"
                  aria-label="Refresh skills"
                  title="Read every folder again"
                  disabled={loading}
                  onClick={() => void skills.refreshSkills()}
                >
                  <RefreshIcon size={13} class={loading ? 'spin' : undefined} />
                </button>
              </span>
            </div>
            {data.truncated ? (
              <p class="field-hint">
                The list is cut at <code>max_skills</code>; raise it above to see the rest.
              </p>
            ) : null}
            {total === 0 ? (
              <div class="skills-empty">
                <p class="skills-empty-title">No skills found</p>
                <p>
                  Add one below, or drop a <code>SKILL.md</code> in{' '}
                  <code class="mono">{workspaceRoot?.path ?? 'a scanned folder'}</code> — one skill
                  per folder, with an optional frontmatter block naming and describing it.
                </p>
              </div>
            ) : (
              <div class="skills-list">
                {rows.map((skill) => (
                  <SkillRow
                    key={skill.id}
                    skill={skill}
                    busy={busy}
                    onView={() => open(skill.id, 'view')}
                    onEdit={() => open(skill.id, 'edit')}
                  />
                ))}
              </div>
            )}
          </section>

          <section class="settings-section" aria-labelledby="settings-skills-add">
            <div class="settings-section-head">
              <h3 id="settings-skills-add">Add a skill</h3>
              <span class="settings-section-note">Stored in the settings table, no file needed</span>
            </div>
            <AddSkillForm disabled={disabled} />
          </section>

          <p class="settings-section-note">
            A skill is data, not instructions: the agent is told to read one, and none of it can
            change the harness rules.
          </p>
        </>
      )}

      {dialog !== null ? <SkillDialog mode={dialog} /> : null}
    </div>
  );
}
