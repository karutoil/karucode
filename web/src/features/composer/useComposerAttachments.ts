import type { RefObject } from 'preact';
import { useRef, useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import {
  classifyFile,
  newAttachmentId,
  readText,
  UPLOAD_MAX_BYTES,
  type Attachment,
} from '../../lib/attachments';
import { formatBytes } from '../../lib/format';
import * as actions from '../../state/actions';
import * as store from '../../state/store';
import { defaultUploadFile, hasFiles } from './helpers';
import type { FieldUpdate, UploadContext } from './types';

export interface ComposerAttachmentsHandle {
  dragging: boolean;
  fileInput: RefObject<HTMLInputElement | null>;
  addFiles: (files: readonly File[]) => Promise<void>;
  dropAttachment: (id: string) => void;
  onDragEnter: (event: DragEvent) => void;
  onDragOver: (event: DragEvent) => void;
  onDragLeave: () => void;
  onDrop: (event: DragEvent) => void;
  onPaste: (event: ClipboardEvent) => void;
}

export function useComposerAttachments(options: {
  sessionId: string | null;
  projectId: string | null;
  canType: boolean;
  disabledReason: string | null;
  attachments: Attachment[];
  setAttachments: (update: FieldUpdate<Attachment[]>) => void;
  setAnnouncement: (update: FieldUpdate<string>) => void;
  uploadFile?: (file: File, context: UploadContext) => Promise<{ path: string }>;
  onUploaded: (path: string, size: number) => void;
}): ComposerAttachmentsHandle {
  const {
    sessionId,
    projectId,
    canType,
    disabledReason,
    attachments,
    setAttachments,
    setAnnouncement,
    uploadFile,
    onUploaded,
  } = options;

  const [dragging, setDragging] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const dragDepth = useRef(0);

  const updateAttachment = (id: string, patch: Partial<Attachment>): void => {
    setAttachments((current) =>
      current.map((attachment) => (attachment.id === id ? { ...attachment, ...patch } : attachment)),
    );
  };

  const processFile = async (
    attachment: Attachment,
    file: File,
    upload: typeof defaultUploadFile,
  ): Promise<void> => {
    if (attachment.mode === 'inline') {
      try {
        const content = await readText(file);
        updateAttachment(attachment.id, { status: 'ready', content });
      } catch {
        updateAttachment(attachment.id, { status: 'error', error: 'Could not read the file' });
      }
      return;
    }
    try {
      // The ids are captured before classification, file reads and the upload.
      // Reading them after an await can upload A's file into B's session.
      //
      // An unsent draft has no id yet, and an uploaded file lives under the
      // session's own directory (`POST /api/workspace/upload?session=`), so the
      // conversation is created here. Attaching a real file is deliberate work,
      // and a chip that cannot be sent would block the composer.
      const target = sessionId ?? (await actions.materializeDraftSession());
      if (!target) throw new Error('Open a session before attaching files.');
      const result = await upload(file, {
        session: target,
        project: projectId,
      });
      updateAttachment(attachment.id, { status: 'ready', path: result.path });
      // The uploaded file is now mentionable in its own workspace. A completion
      // that arrives after a project switch must not seed this project's index.
      if (store.currentProjectId.value === projectId) {
        onUploaded(result.path, attachment.size);
      }
    } catch (error) {
      updateAttachment(attachment.id, { status: 'error', error: toErrorMessage(error) });
    }
  };

  const addFiles = async (files: readonly File[]): Promise<void> => {
    if (files.length === 0) return;
    const accepted: { attachment: Attachment; file: File }[] = [];
    for (const file of files) {
      const decision =
        file.size > UPLOAD_MAX_BYTES
          ? { mode: 'upload' as const, reason: `Larger than ${formatBytes(UPLOAD_MAX_BYTES)}` }
          : await classifyFile(file).catch(() => ({
              mode: 'upload' as const,
              reason: 'Could not read the file',
            }));
      const attachment: Attachment = {
        id: newAttachmentId(file.name),
        name: file.name,
        size: file.size,
        mode: decision.mode,
        path: file.name,
        status:
          file.size > UPLOAD_MAX_BYTES
            ? 'error'
            : decision.mode === 'inline'
              ? 'reading'
              : 'uploading',
        ...(file.size > UPLOAD_MAX_BYTES ? { error: decision.reason } : {}),
      };
      accepted.push({ attachment, file });
    }
    setAttachments((current) => [...current, ...accepted.map((entry) => entry.attachment)]);
    setAnnouncement(
      accepted.length === 1
        ? `Attached ${accepted[0]?.attachment.name ?? 'file'}.`
        : `Attached ${accepted.length} files.`,
    );
    await Promise.all(
      accepted
        .filter((entry) => entry.attachment.status !== 'error')
        .map((entry) => processFile(entry.attachment, entry.file, uploadFile ?? defaultUploadFile)),
    );
  };

  const dropAttachment = (id: string): void => {
    const removed = attachments.find((attachment) => attachment.id === id);
    setAttachments((current) => current.filter((attachment) => attachment.id !== id));
    if (removed) setAnnouncement(`Removed ${removed.name}.`);
  };

  const resetDrag = (): void => {
    dragDepth.current = 0;
    setDragging(false);
  };

  const onDragEnter = (event: DragEvent): void => {
    if (!hasFiles(event)) return;
    // Even a disabled composer must cancel a file drag. Otherwise enabling
    // the drop later can still fall through to browser navigation.
    event.preventDefault();
    if (!canType) return;
    dragDepth.current += 1;
    setDragging(true);
  };

  const onDragOver = (event: DragEvent): void => {
    if (!hasFiles(event)) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = canType ? 'copy' : 'none';
    if (!canType) return;
  };

  const onDragLeave = (): void => {
    // Some browsers report empty `types` on dragleave, so the counter — not
    // the payload — decides when the overlay goes away.
    if (dragDepth.current === 0) return;
    dragDepth.current -= 1;
    if (dragDepth.current === 0) setDragging(false);
  };

  const onDrop = (event: DragEvent): void => {
    if (!hasFiles(event)) return;
    // Cancel every file drop, including when the composer is disabled:
    // ignoring it would make the browser open the dropped local file.
    event.preventDefault();
    const files = event.dataTransfer?.files;
    resetDrag();
    if (!canType) return;
    if (files && files.length > 0) void addFiles(Array.from(files));
  };

  const onPaste = (event: ClipboardEvent): void => {
    const files = event.clipboardData?.files;
    if (!files || files.length === 0) return;
    // A clipboard that carries files nearly always carries their names
    // as text too (copy a file in a file manager and both forms
    // arrive), so the paste has to be claimed: without this the chip
    // *and* "shot.png" typed into the draft are the same paste twice.
    // The paste is only claimed when it is actually used: claiming it on
    // the refusal path suppressed the text the clipboard also carries
    // and then dropped the whole gesture, so the reader got nothing
    // *and* lost the browser default.
    if (!canType) {
      // A dropped paste looks exactly like a broken clipboard, so say
      // which it is - the same live region a refused send uses.
      setAnnouncement(
        `Files cannot be attached while the composer is unavailable${
          disabledReason ? `: ${disabledReason}` : '.'
        }`,
      );
      return;
    }
    event.preventDefault();
    void addFiles(Array.from(files));
  };

  return {
    dragging,
    fileInput,
    addFiles,
    dropAttachment,
    onDragEnter,
    onDragOver,
    onDragLeave,
    onDrop,
    onPaste,
  };
}
