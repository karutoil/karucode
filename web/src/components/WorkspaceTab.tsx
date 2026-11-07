import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import type { WorkspaceEntry, WorkspaceFile } from '../api/types';
import { getWorkspaceFile, getWorkspaceTree } from '../api/endpoints';
import { toErrorMessage } from '../api/client';
import { copyText } from '../lib/clipboard';
import { fileExtension, fileName } from '../lib/fileKind';
import { formatBytes } from '../lib/format';
import { highlightCode, languageLabel } from '../lib/highlight';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { FileTree } from './FileTree';
import {
  ArrowRightIcon,
  CheckIcon,
  CopyIcon,
  FileCodeIcon,
  FolderIcon,
  RefreshIcon,
} from './icons';

type LoadState = 'loading' | 'ready' | 'error';

// Keep the explorer tidy even if the server's listing includes build/VCS dirs.
const HIDDEN_DIRS = new Set(['.git', 'node_modules', 'target']);

/** The mock previews eight evenly spaced lines; larger files are opened instead. */
const PREVIEW_LINES = 8;
const PREVIEW_MAX_BYTES = 512 * 1024;

function visibleEntries(entries: WorkspaceEntry[]): WorkspaceEntry[] {
  return entries.filter((entry) => !(entry.kind === 'dir' && HIDDEN_DIRS.has(entry.name)));
}

function fileExt(path: string): string {
  // An extensionless or dotfile name has no extension: the whole name is the stem.
  const extension = fileExtension(path);
  return extension === '' ? 'file' : extension;
}

/** `rust` → `Rust`: the mock's preview footer names the language, capitalised. */
function languageName(language: string | null): string {
  const label = languageLabel(language);
  return label.charAt(0).toUpperCase() + label.slice(1);
}

type PreviewState =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'ready'; file: WorkspaceFile }
  | { kind: 'too-large'; size: number }
  | { kind: 'error' };

/**
 * The mock's "quick preview" card. It shows the first lines of the file the user
 * last picked in the tree — real bytes, no placeholder sample — and falls back
 * to the file viewer when the content cannot be previewed inline.
 */
function QuickPreview({
  path,
  projectId,
  onOpen,
}: {
  path: string | null;
  projectId: string | null;
  onOpen: () => void;
}): JSX.Element | null {
  const [state, setState] = useState<PreviewState>({ kind: 'idle' });
  const refreshTick = store.workspaceRefreshTick.value;

  useEffect(() => {
    if (!path) {
      setState({ kind: 'idle' });
      return;
    }
    const controller = new AbortController();
    setState({ kind: 'loading' });
    getWorkspaceFile(path, projectId, { signal: controller.signal })
      .then((file) => {
        if (controller.signal.aborted) return;
        if (!file.binary && file.content.length > PREVIEW_MAX_BYTES) {
          setState({ kind: 'too-large', size: file.size });
        } else {
          setState({ kind: 'ready', file });
        }
      })
      .catch(() => {
        if (!controller.signal.aborted) setState({ kind: 'error' });
      });
    return () => controller.abort();
    // A save from the file viewer bumps the tick: this card must not keep showing
    // the pre-save bytes. The same path in another project is a different file.
  }, [path, projectId, refreshTick]);

  const file = state.kind === 'ready' ? state.file : null;
  const lines = useMemo(() => {
    if (!file || file.binary) return [];
    return file.content.split('\n').slice(0, PREVIEW_LINES);
  }, [file]);
  const highlighted = useMemo(() => {
    if (!file || file.binary || lines.length === 0) return '';
    return highlightCode(lines.join('\n'), file.language, 4000);
  }, [file, lines]);
  const lineCount = file && !file.binary ? file.content.split('\n').length : 0;

  if (!path) {
    // The mock always has a file selected; we do not, so keep the same card and
    // let it explain itself rather than hiding the section entirely.
    return (
      <>
        <div class="inspector-inline-head">
          <span class="inspector-kicker">Quick preview</span>
        </div>
        <div class="preview-panel preview-panel-empty">
          <div class="preview-empty">Select a file in the tree to preview it here.</div>
        </div>
      </>
    );
  }

  return (
    <>
      <div class="inspector-inline-head">
        <span class="inspector-kicker">Quick preview</span>
        <button type="button" class="tiny-link" onClick={onOpen}>
          Open <ArrowRightIcon size={12} />
        </button>
      </div>

      <div class="preview-panel">
        <div class="preview-head">
          <FileCodeIcon size={14} />
          <span class="preview-name" title={path}>
            {fileName(path)}
          </span>
          <span class="preview-ext">{fileExt(path)}</span>
        </div>

        {state.kind === 'loading' ? <p class="preview-placeholder">Reading preview…</p> : null}
        {state.kind === 'error' ? (
          <p class="preview-placeholder" role="status">
            The file could not be read for this preview. Open it to see the error.
          </p>
        ) : null}
        {state.kind === 'too-large' ? (
          <p class="preview-placeholder">
            {formatBytes(state.size)} is too large for an inline preview. Open the file instead.
          </p>
        ) : null}
        {file?.binary ? (
          <p class="preview-placeholder">Binary file — no text preview.</p>
        ) : null}
        {file && !file.binary ? (
          <pre class="preview-code">
            <code class="hljs" dangerouslySetInnerHTML={{ __html: highlighted }} />
          </pre>
        ) : null}

        <div class="preview-foot">
          <span>
            {file && !file.binary
              ? `${languageName(file.language)} · ${lineCount} ${lineCount === 1 ? 'line' : 'lines'}`
              : 'Text preview'}
          </span>
          <span>
            {file ? (
              <>
                <CheckIcon size={12} /> {formatBytes(file.size)}
              </>
            ) : (
              'On demand'
            )}
          </span>
        </div>
      </div>
    </>
  );
}

export function WorkspaceTab(): JSX.Element {
  const [rootEntries, setRootEntries] = useState<WorkspaceEntry[]>([]);
  const [children, setChildren] = useState<Record<string, WorkspaceEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [loadingPaths, setLoadingPaths] = useState<Set<string>>(new Set());
  const [state, setState] = useState<LoadState>('loading');
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [lastOpened, setLastOpened] = useState<string | null>(null);
  const [partialPaths, setPartialPaths] = useState<Set<string>>(new Set());
  const treeGeneration = useRef(0);

  // Read the project into a local so the effect re-runs on a project switch;
  // otherwise the previous project's tree would stay on screen.
  const projectId = store.currentProjectId.value;

  useEffect(() => {
    const controller = new AbortController();
    const generation = ++treeGeneration.current;
    setState('loading');
    setError(null);
    setChildren({});
    setExpanded(new Set());
    setLoadingPaths(new Set());
    setPartialPaths(new Set());
    getWorkspaceTree('.', 1, projectId, { signal: controller.signal })
      .then((tree) => {
        if (controller.signal.aborted || generation !== treeGeneration.current) return;
        setRootEntries(visibleEntries(tree.entries));
        setPartialPaths(new Set(tree.truncated ? ['.'] : []));
        setState('ready');
      })
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        setError(toErrorMessage(cause));
        setState('error');
      });
    return () => controller.abort();
    // `refreshKey` is the explicit button; `workspaceRefreshTick` is the viewer
    // reporting a save it made itself.
  }, [refreshKey, projectId, store.workspaceRefreshTick.value]);

  const loadChildren = async (path: string): Promise<void> => {
    if (children[path] || loadingPaths.has(path)) return;
    const generation = treeGeneration.current;
    setLoadingPaths((current) => new Set(current).add(path));
    try {
      const tree = await getWorkspaceTree(path, 1, projectId);
      if (generation !== treeGeneration.current) return;
      setChildren((current) => ({ ...current, [path]: visibleEntries(tree.entries) }));
      setPartialPaths((current) => {
        const next = new Set(current);
        if (tree.truncated) next.add(path);
        else next.delete(path);
        return next;
      });
    } catch (cause) {
      if (generation !== treeGeneration.current) return;
      store.pushToast(toErrorMessage(cause));
    } finally {
      if (generation === treeGeneration.current) {
        setLoadingPaths((current) => {
          const next = new Set(current);
          next.delete(path);
          return next;
        });
      }
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

  const workspace =
    store.projects.value.find((project) => project.id === projectId)?.path ??
    store.health.value?.workspace ??
    store.config.value?.workspace ??
    '.';

  const copyPath = (): void => {
    void copyText(workspace).then((ok) => {
      if (ok) store.pushToast('Workspace path copied', 'success', 2600);
      else store.pushToast('Could not copy the workspace path.', 'error');
    });
  };

  return (
    <div class="inspector-section">
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">Explorer</span>
        <button
          type="button"
          class="icon-button inspector-mini"
          aria-label="Refresh file tree"
          title="Refresh file tree"
          disabled={state === 'loading'}
          onClick={() => setRefreshKey((key) => key + 1)}
        >
          <RefreshIcon size={13} class={state === 'loading' ? 'spin' : undefined} />
        </button>
      </div>

      <div class="workspace-path" title={workspace}>
        <span class="path-icon" aria-hidden="true">
          <FolderIcon size={13} />
        </span>
        <span>{workspace}</span>
        <button
          type="button"
          class="tiny-link"
          aria-label="Copy workspace path"
          title="Copy workspace path"
          onClick={copyPath}
        >
          <CopyIcon size={12} />
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
        <FileTree
          rootPath={workspace}
          rootEntries={rootEntries}
          children={children}
          expanded={expanded}
          loading={loadingPaths}
          onToggleDir={toggleDir}
          onOpenFile={(path) => {
            setLastOpened(path);
            actions.openFileViewer(path);
          }}
        />
      ) : null}

      {partialPaths.size > 0 ? (
        <p class="inspector-note tree-partial-warning" role="status">
          Some folders are only partially listed because the server's entry or traversal budget was
          reached. Use file search for a file that is not shown.
        </p>
      ) : null}

      <QuickPreview
        key={`${projectId ?? 'none'}:${lastOpened ?? ''}`}
        path={lastOpened}
        projectId={projectId}
        onOpen={() => lastOpened && actions.openFileViewer(lastOpened)}
      />

      {state === 'ready' ? (
        <p class="inspector-note">VCS and build directories are hidden. File contents are read on demand.</p>
      ) : null}
    </div>
  );
}
