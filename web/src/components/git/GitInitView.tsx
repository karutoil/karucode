import type { JSX } from 'preact';
import { useId, useState } from 'preact/hooks';
import { AlertIcon, BranchIcon, GitIcon } from '../icons';
import { branchNameProblem } from './gitModel';

export interface GitInitViewProps {
  error: string | null;
  busy: boolean;
  onInit: (options: {
    default_branch?: string;
    initial_commit?: boolean;
    commit_message?: string;
    gitignore_template?: string;
  }) => Promise<boolean>;
}

const GITIGNORE_OPTIONS = [
  { id: 'none', label: 'None' },
  { id: 'rust', label: 'Rust' },
  { id: 'node', label: 'Node / TypeScript' },
  { id: 'python', label: 'Python' },
  { id: 'go', label: 'Go' },
  { id: 'general', label: 'General (.env, logs)' },
];

/**
 * Repository setup surface shown when the project directory is not yet a git
 * repository. Lets the user run `git init` with a chosen default branch,
 * optional `.gitignore` template, and optional initial commit.
 */
export function GitInitView({ error, busy, onInit }: GitInitViewProps): JSX.Element {
  const branchId = useId();
  const templateId = useId();
  const messageId = useId();
  const [defaultBranch, setDefaultBranch] = useState('main');
  const [template, setTemplate] = useState('none');
  const [initialCommit, setInitialCommit] = useState(true);
  const [commitMessage, setCommitMessage] = useState('chore: initial commit');

  const branchProblem = branchNameProblem(defaultBranch);

  const submit = (event: Event): void => {
    event.preventDefault();
    if (busy || branchProblem !== null) return;
    void onInit({
      default_branch: defaultBranch.trim(),
      gitignore_template: template === 'none' ? undefined : template,
      initial_commit: initialCommit,
      commit_message: initialCommit ? commitMessage.trim() || 'chore: initial commit' : undefined,
    });
  };

  if (error) {
    return (
      <div class="empty-inspector" key="not-repo-err">
        <p class="empty-inspector-title">Git unavailable</p>
        <p>The git status probe failed, so there is no working tree to manage here.</p>
        <p class="muted small">
          <AlertIcon size={12} /> {error}
        </p>
      </div>
    );
  }

  return (
    <div class="gitm-setup" key="not-repo">
      <div class="gitm-setup-card">
        <div class="gitm-setup-head">
          <span class="gitm-setup-icon" aria-hidden="true">
            <GitIcon size={20} />
          </span>
          <div>
            <h3 class="empty-inspector-title">Not a git repository</h3>
            <p class="gitm-setup-sub">
              Run <code>git init</code> in this project to track changes, manage branches, sync
              remotes, and connect GitHub pull requests and issues.
            </p>
          </div>
        </div>

        <form class="gitm-setup-form" onSubmit={submit}>
          <div class="gitm-field-grid">
            <div class="gitm-field">
              <label class="gitm-label" for={branchId}>
                Default branch
              </label>
              <div class="gitm-input-icon">
                <BranchIcon size={13} />
                <input
                  id={branchId}
                  type="text"
                  class="gitm-input"
                  value={defaultBranch}
                  disabled={busy}
                  onInput={(event) => setDefaultBranch((event.target as HTMLInputElement).value)}
                />
              </div>
              {branchProblem ? (
                <p class="gitm-field-error" role="status">
                  {branchProblem}
                </p>
              ) : null}
            </div>

            <div class="gitm-field">
              <label class="gitm-label" for={templateId}>
                .gitignore template
              </label>
              <select
                id={templateId}
                class="gitm-select"
                value={template}
                disabled={busy}
                onChange={(event) => setTemplate((event.target as HTMLSelectElement).value)}
              >
                {GITIGNORE_OPTIONS.map((opt) => (
                  <option key={opt.id} value={opt.id}>
                    {opt.label}
                  </option>
                ))}
              </select>
            </div>
          </div>

          <label class="gitm-check">
            <input
              type="checkbox"
              checked={initialCommit}
              disabled={busy}
              onChange={(event) => setInitialCommit((event.target as HTMLInputElement).checked)}
            />
            <span>Create initial commit with existing files</span>
          </label>

          {initialCommit ? (
            <div class="gitm-field">
              <label class="gitm-label" for={messageId}>
                Initial commit message
              </label>
              <input
                id={messageId}
                type="text"
                class="gitm-input"
                value={commitMessage}
                disabled={busy}
                onInput={(event) => setCommitMessage((event.target as HTMLInputElement).value)}
              />
            </div>
          ) : null}

          <div class="gitm-setup-actions">
            <button
              type="submit"
              class="button button-primary"
              disabled={busy || branchProblem !== null}
            >
              Initialize repository
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
