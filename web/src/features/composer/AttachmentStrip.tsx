import type { JSX } from 'preact';
import { CloseIcon, FileIcon } from '../../components/icons';
import { attachmentSummary, type Attachment } from '../../lib/attachments';

export function AttachmentStrip({
  attachments,
  onRemove,
}: {
  attachments: readonly Attachment[];
  onRemove: (id: string) => void;
}): JSX.Element | null {
  if (attachments.length === 0) return null;
  return (
    <ul class="composer-attachments" aria-label="Attached files">
      {attachments.map((attachment) => (
        <li
          key={attachment.id}
          class={`attachment-chip attachment-chip-${attachment.status}`}
          data-mode={attachment.mode}
        >
          <span class="attachment-chip-icon" aria-hidden="true">
            <FileIcon size={12} />
          </span>
          <span class="attachment-chip-body">
            <span class="attachment-chip-name" title={attachment.path}>
              {attachment.name}
            </span>
            <span class="attachment-chip-meta">{attachmentSummary(attachment)}</span>
          </span>
          <button
            type="button"
            class="attachment-chip-remove"
            onClick={() => onRemove(attachment.id)}
            aria-label={`Remove ${attachment.name}`}
          >
            <CloseIcon size={11} />
          </button>
        </li>
      ))}
    </ul>
  );
}
