import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import type { WorkspaceEntry } from '../api/types';
import { getWorkspaceTree } from '../api/endpoints';
import { toErrorMessage } from '../api/client';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { FileTree } from './FileTree';
import { RefreshIcon } from './icons';

type LoadState = 'loading' | 'ready' | 'error';

// Keep the explorer tidy even if the server's listing includes build/VCS dirs.
const HIDDEN_DIRS = new Set(['.git', 'node_modules', 'target']);

function visibleEntries(entries: WorkspaceEntry[]): WorkspaceEntry[] {
  return entries.filter((entry) => !(entry.kind === 'dir' && HIDDEN_DIRS.has(entry.name)));
}

export function WorkspaceTab(): JSX.Element {
  const [rootEntries, setRootEntries] = useState<WorkspaceEntry[]>([]);
  const [children, setChildren] = useState<Record<string, WorkspaceEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [loadingPaths, setLoadingPaths] = useState<Set<string>>(new Set());
  const [state, setState] = useState<LoadState>('loading');
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    setState('loading');
    setError(null);
    setChildren({});
    setExpanded(new Set());
    getWorkspaceTree('.', 1, { signal: controller.signal })
      .then((tree) => {
        if (controller.signal.aborted) return;
        setRootEntries(visibleEntries(tree.entries));
        setState('ready');
      })
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        setError(toErrorMessage(cause));
        setState('error');
      });
    return () => controller.abort();
  }, [refreshKey]);

  const loadChildren = async (path: string): Promise<void> => {
    if (children[path] || loadingPaths.has(path)) return;
    setLoadingPaths((current) => new Set(current).add(path));
    try {
      const tree = await getWorkspaceTree(path, 1);
      setChildren((current) => ({ ...current, [path]: visibleEntries(tree.entries) }));
    } catch (cause) {
      store.pushToast(toErrorMessage(cause));
    } finally {
      setLoadingPaths((current) => {
        const next = new Set(current);
        next.delete(path);
        return next;
      });
    }
  };

  const toggleDir = (entry: WorkspaceEntry): void => {
    const willOpen = !expanded.has(entry.path);
    setExpanded((current) => {
      const next = new Set(current);
      if (willOpen) next.add(entry.path);
      else next.delete(entry.path);
      return next;
    });
    if (willOpen) void loadChildren(entry.path);
  };

  const workspace = store.health.value?.workspace ?? store.config.value?.workspace ?? '.';

  return (
    <div class="inspector-section">
      <div class="section-toolbar">
        <div class="section-heading">
          <span class="eyebrow">Workspace</span>
          <span class="section-path" title={workspace}>
            {workspace}
          </span>
        </div>
        <button
          type="button"
          class="icon-button"
          aria-label="Refresh file tree"
          title="Refresh file tree"
          disabled={state === 'loading'}
          onClick={() => setRefreshKey((key) => key + 1)}
        >
          <RefreshIcon class={state === 'loading' ? 'spin' : undefined} />
        </button>
      </div>

      {state === 'loading' ? (
        <div class="tree-loading-skeleton" aria-busy="true" aria-label="Loading file tree">
          {[0, 1, 2, 3, 4].map((index) => (
            <div class="skeleton skeleton-tree-row" key={index} />
          ))}
        </div>
      ) : null}

      {state === 'error' ? (
        <div class="inline-error" role="alert">
          <p>{error}</p>
          <button type="button" class="button button-small" onClick={() => setRefreshKey((key) => key + 1)}>
            Retry
          </button>
        </div>
      ) : null}

      {state === 'ready' ? (
        <>
          <FileTree
            rootPath={workspace}
            rootEntries={rootEntries}
            children={children}
            expanded={expanded}
            loading={loadingPaths}
            onToggleDir={toggleDir}
            onOpenFile={actions.openFileViewer}
          />
          <p class="inspector-note">
            VCS and build directories are hidden. File contents are read on demand.
          </p>
        </>
      ) : null}
    </div>
  );
}