import { apiRequest, withQuery } from '../client';
import type {
  ArtifactCreateRequest,
  ArtifactCreateResponse,
  AttachmentUploadRequest,
  AttachmentUploadResponse,
  WorkspaceFile,
  WorkspaceFileWrite,
  WorkspaceTree,
} from '../types/workspace';
import type { CallOptions } from './common';

export function getWorkspaceTree(
  path = '.',
  depth = 1,
  project?: string | null,
  options: CallOptions = {},
): Promise<WorkspaceTree> {
  return apiRequest<WorkspaceTree>(
    withQuery('/api/workspace/tree', { path, depth, project: project ?? undefined }),
    options,
  );
}

export function getWorkspaceFile(
  path: string,
  project?: string | null,
  options: CallOptions = {},
): Promise<WorkspaceFile> {
  return apiRequest<WorkspaceFile>(
    withQuery('/api/workspace/file', { path, project: project ?? undefined }),
    options,
  );
}

/**
 * Save a workspace file from the viewer's editor.
 *
 * `PUT /api/workspace/file?path=<file>[&project=][&create=][&force=]` takes the
 * **whole** new contents as JSON. `expectedDigest` is the precise optimistic
 * concurrency check: when the file changed on disk since it was loaded the save
 * is refused with `409` and nothing is written, even if its byte length is still
 * the same. `expectedSize` rides along for compatibility with older servers.
 * `force` skips both checks, and is the user's answer to the overwrite prompt,
 * so only pass it after asking.
 */
export function saveWorkspaceFile(
  path: string,
  content: string,
  params: {
    project?: string | null;
    /** Full-file digest returned by the read, for example `sha256:<64 hex>`. */
    expectedDigest?: string;
    /** Size the editor loaded; retained for compatibility with older servers. */
    expectedSize?: number;
    /** Create the file if it does not exist (the viewer's "new file" path). */
    create?: boolean;
    /** Overwrite even when the file changed on disk. */
    force?: boolean;
  } = {},
  options: CallOptions = {},
): Promise<WorkspaceFileWrite> {
  return apiRequest<WorkspaceFileWrite>(
    withQuery('/api/workspace/file', {
      path,
      project: params.project ?? undefined,
      create: params.create ? 'true' : undefined,
      force: params.force ? 'true' : undefined,
    }),
    {
      ...options,
      method: 'PUT',
      body: {
        content,
        expected_digest: params.expectedDigest,
        expected_size: params.expectedSize,
      },
    },
  );
}

/**
 * URL for one workspace file's raw bytes (`GET /api/workspace/raw`).
 *
 * The text endpoint answers JSON, which an `<img>` or `<iframe>` cannot render,
 * so the viewer points those elements at this URL. It is read-only and confined
 * like every other workspace path.
 */
export function workspaceRawUrl(path: string, project?: string | null): string {
  const query = new URLSearchParams({ path });
  if (project) query.set('project', project);
  return `/api/workspace/raw?${query.toString()}`;
}

/**
 * URL for one workspace HTML file rendered as a **document** (`GET
 * /api/workspace/preview/<path>` or
 * `/api/workspace/preview-project/<project>/<path>`).
 *
 * The path is in the URL, not a query parameter, because the page resolves its
 * own relative references (`./style.css`, `../img/logo.png`, `./app.js`) against
 * it. A `srcdoc` frame cannot do that, and it inherits this app's own
 * `default-src 'self'` policy while running in an opaque origin where `'self'
 * matches nothing - so its inline scripts, sibling stylesheets and relative
 * images were all refused and the preview came up nearly blank.
 *
 * The project is in the **path** for the same reason. A relative reference drops
 * the query string, so `?project=<id>` scoped only the entry document: the page's
 * own `style.css` was requested without a project, resolved against the default
 * workspace and came back 404 (measured in Chromium - the preview rendered its
 * HTML and none of its assets).
 *
 * Each segment is encoded separately: a path with a space or `#` must survive
 * the trip, but a literal `/` is a directory separator the browser has to see.
 */
export function workspacePreviewUrl(path: string, project?: string | null): string {
  const encoded = path
    .split('/')
    .filter((segment) => segment !== '')
    .map((segment) => encodeURIComponent(segment))
    .join('/');
  if (!project) return `/api/workspace/preview/${encoded}`;
  return `/api/workspace/preview-project/${encodeURIComponent(project)}/${encoded}`;
}

/**
 * Publish one chat artifact and get back the URL that renders it.
 *
 * `POST /api/artifacts` takes the artifact's source (HTML or a fragment of it,
 * already stripped of any fence metadata) and answers the id and URL of a stored
 * document. It is content-addressed, so posting the same artifact again - every
 * page load, every re-render - is one entry, and the id is stable.
 *
 * The `theme` decides which way the wrapper's own defaults go for a *fragment*:
 * the card passes the app's mode so a dark transcript does not get a white
 * rectangle. A complete document (one with its own `<html>`) is served verbatim
 * and ignores it.
 */
export function createArtifact(
  body: ArtifactCreateRequest,
  options: CallOptions = {},
): Promise<ArtifactCreateResponse> {
  return apiRequest<ArtifactCreateResponse>('/api/artifacts', {
    ...options,
    method: 'POST',
    body,
  });
}

// Artifact frame URLs come directly from `createArtifact(...).url`. They carry
// a random per-artifact read capability; never put the master bearer in a URL
// whose document executes untrusted inline JavaScript.

/** Scope for an upload: the session owns the directory, the project the root. */
export interface UploadAttachmentParams {
  session: string;
  project?: string | null;
}

/**
 * Store a local file in the workspace so the agent can open it.
 *
 * `POST /api/workspace/upload?session=<id>&name=<file>[&project=<id>]` takes
 * the raw bytes as the request body and answers `201` with the workspace-relative
 * path under `.karucode/attachments/<session>/` that the message must reference.
 * Names are never overwritten, so the response may report `report-1.pdf` where
 * `report.pdf` was sent: always compose the reference from the RESPONSE, never
 * from the local `File.name`.
 */
export function uploadAttachment(
  file: AttachmentUploadRequest,
  params: UploadAttachmentParams,
  options: CallOptions = {},
): Promise<AttachmentUploadResponse> {
  return apiRequest<AttachmentUploadResponse>(
    withQuery('/api/workspace/upload', {
      session: params.session,
      name: file.name,
      project: params.project ?? undefined,
    }),
    {
      ...options,
      method: 'POST',
      rawBody: file.data,
      contentType: file.contentType,
    },
  );
}
