import type { JSX } from 'preact';
import type { QueuedKind } from '../../api/types';
import { AlertIcon, LockIcon } from '../../components/icons';
import { describeApprovals } from '../../lib/approvalCopy';
import type { Activity } from '../../state/selectors';
import * as store from '../../state/store';

export interface ComposerFooterProps {
  status: Activity;
  disabledReason: string | null;
  running: boolean;
  queueMode: QueuedKind;
  sendChord: string;
  mentionLabel?: string;
  usageLabel?: string;
  onOpenSettings: () => void;
}

export function ComposerFooter({
  status,
  disabledReason,
  running,
  queueMode,
  sendChord,
  mentionLabel,
  usageLabel,
  onOpenSettings,
}: ComposerFooterProps): JSX.Element {
  const approvals = describeApprovals(store.config.value);
  const usageTitle = usageLabel ? `Session usage: ${usageLabel}` : undefined;

  return (
    /* The mock's `.compose-foot` is a sibling of the box, not its last child:
     * status + hints on the left, the approval policy on the right. */
    <div class="compose-foot">
      <span class="compose-foot-main">
        <span
          id="composer-status"
          class={`composer-status status-${status.kind}`}
          role="status"
          aria-live="polite"
          title={usageTitle}
        >
          {disabledReason ? (
            <>
              <AlertIcon size={13} />
              <span>{disabledReason}</span>
              <button type="button" class="link-button" onClick={onOpenSettings}>
                Open settings
              </button>
            </>
          ) : (
            <>
              <span class={`status-dot status-${status.kind}`} aria-hidden="true" />
              <span>{status.label}</span>
            </>
          )}
        </span>

        {disabledReason ? null : (
          <span class="composer-hint" aria-hidden="true">
            {running ? (
              <>
                <strong>{sendChord}</strong> to {queueMode === 'steer' ? 'steer' : 'queue a follow-up'}
                <span class="composer-hint-sep">·</span>
                <strong>Esc</strong> to stop
              </>
            ) : (
              <>
                <strong>{sendChord}</strong> to send
                <span class="composer-hint-sep">·</span>
                <strong>@</strong> reference {mentionLabel ?? 'files'}
              </>
            )}
          </span>
        )}
      </span>

      {approvals ? (
        <button
          type="button"
          class="compose-approvals"
          title={approvals.title}
          aria-label={`Approvals: ${approvals.label}. Open Settings`}
          onClick={onOpenSettings}
        >
          Approvals: {approvals.label}
          <LockIcon size={11} />
        </button>
      ) : null}
    </div>
  );
}
