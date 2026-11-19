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
import { Button, Checkbox, IconButton, Input, Select } from '../../ui';
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
            <IconButton
              label="Refresh remotes"
              title="Refresh remotes"
              testId="git-remotes-refresh"
              icon={<RefreshIcon size={14} />}
              disabled={busy}
              onClick={() => {
                onLoadRemotes();
                onLoadGitHubStatus();
              }}
            />
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
                        <Input
                          type="text"
                          class="gitm-input"
                          aria-label={`URL for ${remote.name}`}
                          value={editing.url}
                          disabled={busy}
                          testId={`git-remote-url-${remote.name}`}
                          onInput={(event) =>
                            setEditing({
                              name: remote.name,
                              url: (event.target as HTMLInputElement).value,
                            })
                          }
                        />
                        <Button
                          type="submit"
                          variant="primary"
                          size="sm"
                          testId="git-remote-url-save"
                          disabled={busy}
                        >
                          Save
                        </Button>
                        <Button
                          variant="secondary"
                          size="sm"
                          testId="git-remote-url-cancel"
                          disabled={busy}
                          onClick={() => setEditing(null)}
                        >
                          Cancel
                        </Button>
                      </form>
                    ) : (
                      <code class="gitm-remote-url">
                        {remote.fetch_url ?? remote.push_url ?? '(no url)'}
                      </code>
                    )}
                  </div>
                  <div class="gitm-remote-actions">
                    <IconButton
                      size="sm"
                      class="gitm-act"
                      label={`Edit URL for ${remote.name}`}
                      title="Edit remote URL"
                      testId={`git-remote-edit-${remote.name}`}
                      icon={<EditIcon size={13} />}
                      disabled={busy}
                      onClick={() =>
                        setEditing({
                          name: remote.name,
                          url: remote.fetch_url ?? remote.push_url ?? '',
                        })
                      }
                    />
                    <IconButton
                      size="sm"
                      class="gitm-act is-danger text-danger"
                      label={`Remove remote ${remote.name}`}
                      title={`Remove ${remote.name}`}
                      testId={`git-remote-remove-${remote.name}`}
                      icon={<TrashIcon size={13} />}
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
                    />
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
                <Input
                  id={nameId}
                  type="text"
                  class="gitm-input"
                  placeholder="origin"
                  value={addName}
                  disabled={busy}
                  testId="git-remote-add-name"
                  onInput={(event) => setAddName((event.target as HTMLInputElement).value)}
                />
              </div>
              <div class="gitm-field">
                <label class="sr-only" for={urlId}>
                  Remote URL
                </label>
                <Input
                  id={urlId}
                  type="text"
                  class="gitm-input"
                  placeholder="https://github.com/owner/repo.git"
                  value={addUrl}
                  disabled={busy}
                  testId="git-remote-add-url"
                  onInput={(event) => setAddUrl((event.target as HTMLInputElement).value)}
                />
              </div>
              <Button
                type="submit"
                variant="primary"
                size="sm"
                icon={<PlusIcon size={13} />}
                testId="git-remote-add-submit"
                disabled={busy || nameProblem !== null || addName.trim() === '' || addUrl.trim() === ''}
              >
                Add
              </Button>
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
                <Checkbox
                  class="gitm-check"
                  checked={fetchPrune}
                  disabled={busy}
                  label="Prune deleted branches on fetch"
                  onChange={(checked) => setFetchPrune(checked)}
                />
                <Checkbox
                  class="gitm-check"
                  checked={pullRebase}
                  disabled={busy}
                  label="Rebase on pull"
                  onChange={(checked) => setPullRebase(checked)}
                />
              </div>
              <div class="gitm-sync-buttons">
                <Button
                  variant="secondary"
                  icon={<RefreshIcon size={13} />}
                  testId="git-fetch-all"
                  disabled={busy}
                  onClick={() => void onFetch({ all: true, prune: fetchPrune })}
                >
                  Fetch all
                </Button>
                <Button
                  variant="secondary"
                  icon={<DownloadIcon size={13} />}
                  testId="git-pull"
                  disabled={busy || !status?.branch}
                  onClick={() => void onPull({ rebase: pullRebase })}
                >
                  Pull{status?.behind ? ` (↓${status.behind})` : ''}
                </Button>
                <Button
                  variant="primary"
                  icon={<UploadIcon size={13} />}
                  testId="git-push"
                  disabled={busy || !status?.branch}
                  onClick={() =>
                    void onPush({
                      set_upstream: !status?.upstream,
                    })
                  }
                >
                  {status?.upstream
                    ? `Push${status.ahead ? ` (↑${status.ahead})` : ''}`
                    : 'Publish branch'}
                </Button>
                {status?.upstream ? (
                  <Button
                    variant="secondary"
                    testId="git-force-push"
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
                  </Button>
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
                <Input
                  id={ghNameId}
                  type="text"
                  class="gitm-input"
                  value={ghName}
                  disabled={busy}
                  testId="git-repo-name"
                  onInput={(event) => setGhName((event.target as HTMLInputElement).value)}
                />
              </div>
              <div class="gitm-field">
                <label class="gitm-label" for={ghDescId}>
                  Description (optional)
                </label>
                <Input
                  id={ghDescId}
                  type="text"
                  class="gitm-input"
                  placeholder="Short description"
                  value={ghDesc}
                  disabled={busy}
                  testId="git-repo-description"
                  onInput={(event) => setGhDesc((event.target as HTMLInputElement).value)}
                />
              </div>
              <div class="gitm-field-grid">
                <div class="gitm-field">
                  <span class="gitm-label">Visibility</span>
                  <Select
                    class="gitm-select"
                    aria-label="Repository visibility"
                    value={ghVisibility}
                    disabled={busy}
                    testId="git-repo-visibility"
                    options={[
                      { value: 'private', label: 'Private' },
                      { value: 'public', label: 'Public' },
                      { value: 'internal', label: 'Internal' },
                    ]}
                    onChange={(event) =>
                      setGhVisibility(
                        (event.target as HTMLSelectElement).value as 'private' | 'public' | 'internal',
                      )
                    }
                  />
                </div>
                <Checkbox
                  class="gitm-check"
                  checked={ghPush}
                  disabled={busy || !status?.has_head}
                  label="Push commits now"
                  onChange={(checked) => setGhPush(checked)}
                />
              </div>
              <div class="gitm-setup-actions">
                <Button
                  type="submit"
                  variant="primary"
                  icon={<CheckIcon size={13} />}
                  testId="git-repo-create"
                  disabled={busy || ghName.trim() === ''}
                >
                  Create GitHub repository
                </Button>
              </div>
            </form>
          ) : null}
        </section>
      </div>
    </div>
  );
}
