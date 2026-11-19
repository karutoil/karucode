export interface WorkspaceEntry {
  name: string;
  path: string;
  kind: 'dir' | 'file' | string;
  size: number;
}

export interface WorkspaceTree {
  path: string;
  entries: WorkspaceEntry[];
  /** Optional for servers predating bounded tree collection. */
  truncated?: boolean;
  /** Maximum entries in this response, separate from the traversal work cap. */
  limit?: number;
}

/**
 * `POST /api/workspace/upload?session=<id>&name=<file>[&project=<id>]`.
 *
 * The request body is the raw file bytes (no base64, no multipart) with
 * `Content-Type: application/octet-stream`; the server stores the bytes
 * verbatim under `<project>/.karucode/attachments/<session>/` so the
 * workspace-confined file tools can read them back.
 */
export interface AttachmentUploadRequest {
  /** Original file name, without directories (`<=160` bytes). */
  name: string;
  /** Raw bytes, at most 8 MiB. */
  data: Blob | ArrayBuffer | Uint8Array;
  /** MIME type; omit for the contract's `application/octet-stream`. */
  contentType?: string;
}

/**
 * `201` response of `POST /api/workspace/upload`.
 *
 * Only these three fields are guaranteed by the contract. The server may send
 * more (`session`, `content_type`, `workspace`, `deduplicated`) — read them only
 * as hints.
 */
export interface AttachmentUploadResponse {
  /** Workspace-relative path — usable in an @mention or a tool argument. */
  path: string;
  /**
   * Name actually written, which can differ from the requested one on a
   * collision (`report.pdf` → `report-1.pdf`). Compose references from `path`.
   */
  name: string;
  /** Bytes written. */
  size: number;
  session?: string;
  content_type?: string;
  /** Absolute project root the file was written under (hint). */
  workspace?: string;
  /** True when the requested name was already taken (hint). */
  deduplicated?: boolean;
}

export interface WorkspaceFile {
  path: string;
  content: string;
  size: number;
  /**
   * SHA-256 of the complete on-disk bytes, as `sha256:<64 lowercase hex>`.
   * Optional only for compatibility with older servers.
   */
  digest?: string;
  truncated: boolean;
  binary: boolean;
  language: string | null;
}

/**
 * `200` response of `PUT /api/workspace/file` — the editor's save.
 *
 * The body sent is `{ content, expected_digest?, expected_size? }`; the digest is
 * the precise optimistic-concurrency check and size remains for older servers.
 * `created` says the save made a new file rather than replacing one.
 */
export interface WorkspaceFileWrite {
  path: string;
  /** Bytes written (`content.length` in UTF-8). */
  size: number;
  /** Fresh digest of the written bytes on servers that support digest checks. */
  digest?: string;
  /** True when the file did not exist before this save. */
  created: boolean;
  language: string | null;
}

/**
 * `POST /api/artifacts` — publish one chat artifact's source.
 *
 * `html` is the artifact body with its fence metadata already stripped; a
 * fragment is wrapped into a document server-side. `theme` only picks which way
 * the wrapper's own background/foreground defaults go (`light` when omitted).
 */
export interface ArtifactCreateRequest {
  html: string;
  theme?: 'light' | 'dark';
}

/**
 * `201` response of `POST /api/artifacts`.
 *
 * `id` is the content hash the document is stored under, and `url` is what an
 * `<iframe>` loads. Both are stable for the same source and theme, so a client
 * can post freely without tracking what it has already published.
 */
export interface ArtifactCreateResponse {
  id: string;
  url: string;
  /** Bytes of the wrapped document that will be served. */
  bytes: number;
  theme: 'light' | 'dark';
}
