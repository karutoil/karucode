import type { JSX } from 'preact';
import { useEffect, useId, useState } from 'preact/hooks';
import type { GitHubStatus, GitRemote, GitStatus } from '../../api/types';
import {
  AlertIcon,
  CheckIcon,
  DownloadIcon,
  EditIcon,
  ExternalIcon,
  GlobeIcon,
  PlusIcon,
  RefreshIcon,
  TrashIcon,
  UploadIcon,
} from '../icons';
import { remoteNameProblem } from './gitModel';
import type { ConfirmRequest } from './GitConfirmDialog';

export interface GitRemotesViewProps {
  status: GitStatus | null;
  remotes: GitRemote[] | null;
  githubStatus: GitHubStatus | null;
  defaultRepoName: string;
  busy: boolean;
  onLoadRemotes: () => void;
  onLoadGitHubStatus: () => void;
  onManageRemote: (body: {
    action: 'add' | 'set_url' | 'remove' | 'rename';
    name: string;
    url?: string;
    new_name?: string;
  }) => Promise<boolean>;
  onFetch: (options?: { remote?: string; prune?: boolean; all?: boolean; tags?: boolean }) => Promise<boolean>;
  onPull: (options?: { remote?: string; branch?: string; rebase?: boolean }) => Promise<boolean>;
  onPush: (options?: {
    remote?: string;
    branch?: string;
    set_upstream?: boolean;
    force_with_lease?: boolean;
    tags?: boolean;
    confirm?: boolean;
  }) => Promise<boolean>;
  onCreateGitHubRepo: (body: {
    name: string;
    description?: string;
    visibility?: 'private' | 'public' | 'internal';
    remote?: string;
    push?: boolean;
  }) => Promise<boolean>;
  onRequestConfirm: (request: ConfirmRequest) => void;
}

export function GitRemotesView({
  status,
  remotes,
  githubStatus,
  defaultRepoName,
  busy,
  onLoadRemotes,
  onLoadGitHubStatus,
  onManageRemote,
  onFetch,
  onPull,
  onPush,
  onCreateGitHubRepo,
  onRequestConfirm,
}: GitRemotesViewProps): JSX.Element {
  const nameId = useId();
  const urlId = useId();
  const ghNameId = useId();
  const ghDescId = useId();

  const [addName, setAddName] = useState('origin');
  const [addUrl, setAddUrl] = useState('');
  const [editing, setEditing] = useState<{ name: string; url: string } | null>(null);

  const [ghName, setGhName] = useState(defaultRepoName);
  const [ghDesc, setGhDesc] = useState('');
  const [ghVisibility, setGhVisibility] = useState<'private' | 'public' | 'internal'>('private');
  const [ghPush, setGhPush] = useState(true);

  const [pullRebase, setPullRebase] = useState(false);
  const [fetchPrune, setFetchPrune] = useState(true);

  useEffect(() => {
    onLoadRemotes();
    onLoadGitHubStatus();
  }, [onLoadRemotes, onLoadGitHubStatus]);

  useEffect(() => {
    setGhName(defaultRepoName);
  }, [defaultRepoName]);

  const remoteList = remotes ?? [];
  const nameProblem = addName.trim() === '' ? null : remoteNameProblem(addName);
  const hasOrigin = remoteList.some((r) => r.name === 'origin');

  const submitAdd = (event: Event): void => {
    event.preventDefault();
    if (busy || nameProblem !== null || addName.trim() === '' || addUrl.trim() === '') return;
    void onManageRemote({
      action: 'add',
      name: addName.trim(),
      url: addUrl.trim(),
    }).then((ok) => {
      if (ok) {
        setAddName(hasOrigin ? 'upstream' : 'origin');
        setAddUrl('');
      }
    });
  };

  const submitEdit = (event: Event): void => {
    event.preventDefault();
    if (!editing || busy || editing.url.trim() === '') return;
    void onManageRemote({
      action: 'set_url',
      name: editing.name,
      url: editing.url.trim(),
    }).then((ok) => {
      if (ok) setEditing(null);
    });
  };

  const submitCreateGh = (event: Event): void => {
    event.preventDefault();
    if (busy || ghName.trim() === '') return;
    void onCreateGitHubRepo({
      name: ghName.trim(),
      description: ghDesc.trim() || undefined,
      visibility: ghVisibility,
      remote: 'origin',
      push: ghPush,
    });
  };

  return (
    <div class="gitm-pane-scroll">
      <div class="gitm-two-col">
        <section class="gitm-card" aria-label="Configured remotes">
          <div class="gitm-card-head">
            <div>
              <h3 class="gitm-card-title">Git remotes</h3>
              <p class="gitm-card-sub">
                {remoteList.length === 0
                  ? 'No remotes configured yet. Add an existing URL or create a GitHub repository.'
                  : `${remoteList.length} remote${remoteList.length === 1 ? '' : 's'} configured`}
              </p>
            </div>
            <button
              type="button"
              class="icon-button"
              aria-label="Refresh remotes"
              title="Refresh remotes"
              disabled={busy}
              onClick={() => {
                onLoadRemotes();
                onLoadGitHubStatus();
              }}
            >
              <RefreshIcon size={14} />
            </button>
          </div>

          {remoteList.length > 0 ? (
            <ul class="gitm-remote-list">
              {remoteList.map((remote) => (
                <li class="gitm-remote-row" key={remote.name}>
                  <div class="gitm-remote-info">
                    <div class="gitm-remote-title-row">
                      <span class="gitm-remote-name">{remote.name}</span>
                      {remote.github ? (
                        <span class="gitm-ref">
                          GitHub · {remote.github.owner}/{remote.github.repo}
                        </span>
                      ) : null}
                    </div>
                    {editing?.name === remote.name ? (
                      <form class="gitm-inline-edit" onSubmit={submitEdit}>
                        <input
                          type="text"
                          class="gitm-input"
                          aria-label={`URL for ${remote.name}`}
                          value={editing.url}
                          disabled={busy}
                          onInput={(event) =>
                            setEditing({
                              name: remote.name,
                              url: (event.target as HTMLInputElement).value,
                            })
                          }
                        />
                        <button type="submit" class="button button-small button-primary" disabled={busy}>
                          Save
                        </button>
                        <button
                          type="button"
                          class="button button-small"
                          disabled={busy}
                          onClick={() => setEditing(null)}
                        >
                          Cancel
                        </button>
                      </form>
                    ) : (
                      <code class="gitm-remote-url">
                        {remote.fetch_url ?? remote.push_url ?? '(no url)'}
                      </code>
                    )}
                  </div>
                  <div class="gitm-remote-actions">
                    <button
                      type="button"
                      class="gitm-act"
                      aria-label={`Edit URL for ${remote.name}`}
                      title="Edit remote URL"
                      disabled={busy}
                      onClick={() =>
                        setEditing({
                          name: remote.name,
                          url: remote.fetch_url ?? remote.push_url ?? '',
                        })
                      }
                    >
                      <EditIcon size={13} />
                    </button>
                    <button
                      type="button"
                      class="gitm-act is-danger"
                      aria-label={`Remove remote ${remote.name}`}
                      title={`Remove ${remote.name}`}
                      disabled={busy}
                      onClick={() =>
                        onRequestConfirm({
                          title: `Remove remote ${remote.name}?`,
                          description: `Local branches and commits stay intact, but ${remote.name} will be disconnected.`,
                          confirmLabel: 'Remove remote',
                          onConfirm: async () => {
                            await onManageRemote({ action: 'remove', name: remote.name });
                          },
                        })
                      }
                    >
                      <TrashIcon size={13} />
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          ) : null}

          <form class="gitm-subform" onSubmit={submitAdd}>
            <h4 class="gitm-label">Add remote</h4>
            <div class="gitm-field-grid">
              <div class="gitm-field">
                <label class="sr-only" for={nameId}>
                  Remote name
                </label>
                <input
                  id={nameId}
                  type="text"
                  class="gitm-input"
                  placeholder="origin"
                  value={addName}
                  disabled={busy}
                  onInput={(event) => setAddName((event.target as HTMLInputElement).value)}
                />
              </div>
              <div class="gitm-field">
                <label class="sr-only" for={urlId}>
                  Remote URL
                </label>
                <input
                  id={urlId}
                  type="text"
                  class="gitm-input"
                  placeholder="https://github.com/owner/repo.git"
                  value={addUrl}
                  disabled={busy}
                  onInput={(event) => setAddUrl((event.target as HTMLInputElement).value)}
                />
              </div>
              <button
                type="submit"
                class="button button-primary"
                disabled={busy || nameProblem !== null || addName.trim() === '' || addUrl.trim() === ''}
              >
                <PlusIcon size={13} /> Add
              </button>
            </div>
            {nameProblem ? (
              <p class="gitm-field-error" role="status">
                {nameProblem}
              </p>
            ) : null}
          </form>

          {remoteList.length > 0 ? (
            <div class="gitm-sync-box">
              <h4 class="gitm-label">Remote sync</h4>
              <div class="gitm-sync-toggles">
                <label class="gitm-check">
                  <input
                    type="checkbox"
                    checked={fetchPrune}
                    disabled={busy}
                    onChange={(event) => setFetchPrune((event.target as HTMLInputElement).checked)}
                  />
                  <span>Prune deleted branches on fetch</span>
                </label>
                <label class="gitm-check">
                  <input
                    type="checkbox"
                    checked={pullRebase}
                    disabled={busy}
                    onChange={(event) => setPullRebase((event.target as HTMLInputElement).checked)}
                  />
                  <span>Rebase on pull</span>
                </label>
              </div>
              <div class="gitm-sync-buttons">
                <button
                  type="button"
                  class="button"
                  disabled={busy}
                  onClick={() => void onFetch({ all: true, prune: fetchPrune })}
                >
                  <RefreshIcon size={13} /> Fetch all
                </button>
                <button
                  type="button"
                  class="button"
                  disabled={busy || !status?.branch}
                  onClick={() => void onPull({ rebase: pullRebase })}
                >
                  <DownloadIcon size={13} /> Pull{status?.behind ? ` (↓${status.behind})` : ''}
                </button>
                <button
                  type="button"
                  class="button button-primary"
                  disabled={busy || !status?.branch}
                  onClick={() =>
                    void onPush({
                      set_upstream: !status?.upstream,
                    })
                  }
                >
                  <UploadIcon size={13} />{' '}
                  {status?.upstream
                    ? `Push${status.ahead ? ` (↑${status.ahead})` : ''}`
                    : 'Publish branch'}
                </button>
                {status?.upstream ? (
                  <button
                    type="button"
                    class="button"
                    disabled={busy}
                    onClick={() =>
                      onRequestConfirm({
                        title: `Force push ${status.branch ?? 'HEAD'}?`,
                        description:
                          'Runs `git push --force-with-lease`. Remote history on this branch will be rewritten if it diverges.',
                        confirmLabel: 'Force push',
                        onConfirm: async () => {
                          await onPush({ force_with_lease: true, confirm: true });
                        },
                      })
                    }
                  >
                    Force push
                  </button>
                ) : null}
              </div>
            </div>
          ) : null}
        </section>

        <section class="gitm-card" aria-label="GitHub repository">
          <div class="gitm-card-head">
            <div>
              <h3 class="gitm-card-title">
                <GlobeIcon size={15} /> GitHub integration
              </h3>
              <p class="gitm-card-sub">
                {githubStatus === null
                  ? 'Checking `gh` CLI status…'
                  : !githubStatus.gh_installed
                    ? '`gh` CLI is not installed on the server.'
                    : !githubStatus.authenticated
                      ? '`gh` CLI is installed, but not signed in.'
                      : `Signed in as @${githubStatus.username ?? 'github'}`}
              </p>
            </div>
          </div>

          {githubStatus && !githubStatus.gh_installed ? (
            <div class="gitm-note-box">
              <p>
                <AlertIcon size={13} /> Install the GitHub CLI (<code>gh</code>) and run{' '}
                <code>gh auth login</code> in the Terminal tab to create GitHub repositories and
                manage Pull Requests &amp; Issues here.
              </p>
            </div>
          ) : githubStatus && !githubStatus.authenticated ? (
            <div class="gitm-note-box">
              <p>
                <AlertIcon size={13} /> Run <code>gh auth login</code> in the Terminal tab to
                authenticate your GitHub account.
              </p>
            </div>
          ) : githubStatus?.repo ? (
            <div class="gitm-gh-repo">
              <div class="gitm-gh-repo-top">
                <strong>{githubStatus.repo.nameWithOwner}</strong>
                <span class="gitm-chip">
                  {githubStatus.repo.isPrivate ? 'Private' : 'Public'}
                </span>
                {githubStatus.repo.defaultBranchRef?.name ? (
                  <span class="gitm-chip">
                    default: {githubStatus.repo.defaultBranchRef.name}
                  </span>
                ) : null}
              </div>
              {githubStatus.repo.description ? (
                <p class="gitm-gh-desc">{githubStatus.repo.description}</p>
              ) : null}
              <div class="gitm-gh-stats">
                {githubStatus.repo.stargazerCount !== undefined ? (
                  <span>{githubStatus.repo.stargazerCount} stars</span>
                ) : null}
                {githubStatus.repo.forkCount !== undefined ? (
                  <span>{githubStatus.repo.forkCount} forks</span>
                ) : null}
                {githubStatus.repo.url ? (
                  <a
                    class="tiny-link"
                    href={githubStatus.repo.url}
                    target="_blank"
                    rel="noreferrer"
                  >
                    Open on GitHub <ExternalIcon size={12} />
                  </a>
                ) : null}
              </div>
            </div>
          ) : githubStatus?.authenticated ? (
            <form class="gitm-subform" onSubmit={submitCreateGh}>
              <h4 class="gitm-label">Create &amp; publish GitHub repository</h4>
              <p class="gitm-hint">
                Creates a new repository on GitHub under your account (or <code>org/name</code>),
                attaches <code>origin</code>, and pushes your branch.
              </p>
              <div class="gitm-field">
                <label class="gitm-label" for={ghNameId}>
                  Repository name
                </label>
                <input
                  id={ghNameId}
                  type="text"
                  class="gitm-input"
                  value={ghName}
                  disabled={busy}
                  onInput={(event) => setGhName((event.target as HTMLInputElement).value)}
                />
              </div>
              <div class="gitm-field">
                <label class="gitm-label" for={ghDescId}>
                  Description (optional)
                </label>
                <input
                  id={ghDescId}
                  type="text"
                  class="gitm-input"
                  placeholder="Short description"
                  value={ghDesc}
                  disabled={busy}
                  onInput={(event) => setGhDesc((event.target as HTMLInputElement).value)}
                />
              </div>
              <div class="gitm-field-grid">
                <div class="gitm-field">
                  <span class="gitm-label">Visibility</span>
                  <select
                    class="gitm-select"
                    aria-label="Repository visibility"
                    value={ghVisibility}
                    disabled={busy}
                    onChange={(event) =>
                      setGhVisibility(
                        (event.target as HTMLSelectElement).value as 'private' | 'public' | 'internal',
                      )
                    }
                  >
                    <option value="private">Private</option>
                    <option value="public">Public</option>
                    <option value="internal">Internal</option>
                  </select>
                </div>
                <label class="gitm-check">
                  <input
                    type="checkbox"
                    checked={ghPush}
                    disabled={busy || !status?.has_head}
                    onChange={(event) => setGhPush((event.target as HTMLInputElement).checked)}
                  />
                  <span>Push commits now</span>
                </label>
              </div>
              <div class="gitm-setup-actions">
                <button
                  type="submit"
                  class="button button-primary"
                  disabled={busy || ghName.trim() === ''}
                >
                  <CheckIcon size={13} /> Create GitHub repository
                </button>
              </div>
            </form>
          ) : null}
        </section>
      </div>
    </div>
  );
}
