import type { JSX } from 'preact';
import type { QueuedKind } from '../../api/types';
import { AlertIcon, LockIcon } from '../../components/icons';
import { describeApprovals } from '../../lib/approvalCopy';
import type { Activity } from '../../state/selectors';
import * as store from '../../state/store';
import { Button, LinkButton } from '../../ui';

export interface ComposerFooterProps {
  status: Activity;
  disabledReason: string | null;
  running: boolean;
  queueMode: QueuedKind;
  sendChord: string;
  mentionLabel?: string;
  usageLabel?: string;
  /**
   * True while a plain `Enter` sends. A device with a soft keyboard then has no
   * way to break a line at all, so the foot offers the one control that does.
   */
  enterSends: boolean;
  /** Insert a newline at the caret; the box stays focused. */
  onLineBreak: () => void;
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
  enterSends,
  onLineBreak,
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
              <LinkButton class="link-button" testId="composer-open-settings" onClick={onOpenSettings}>
                Open settings
              </LinkButton>
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

      {enterSends && !disabledReason ? (
        <Button
          variant="ghost"
          size="sm"
          class="compose-newline"
          aria-label="Start a new line instead of sending"
          testId="compose-newline"
          onClick={onLineBreak}
        >
          ↵ New line
        </Button>
      ) : null}

      {approvals ? (
        <Button
          variant="ghost"
          size="sm"
          class="compose-approvals"
          title={approvals.title}
          aria-label={`Approvals: ${approvals.label}. Open Settings`}
          iconRight={<LockIcon size={11} />}
          testId="composer-approvals"
          onClick={onOpenSettings}
        >
          Approvals: {approvals.label}
        </Button>
      ) : null}
    </div>
  );
}
