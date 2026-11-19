import type { JSX, RefObject } from 'preact';
import type { CommandSpec, QueuedKind } from '../../api/types';
import { ModelControls } from '../../components/ModelSwitcher';
import { PluginSlot } from '../../components/PluginSlot';
import {
  isUiHidden,
  UI_HIDE_COMPOSER_ATTACH,
  UI_HIDE_COMPOSER_MODEL,
  UI_SLOT_COMPOSER_TOOLBAR,
} from '../../components/uiContributions';
import { ArrowUpIcon, PaperclipIcon, StopIcon } from '../../components/icons';
import * as store from '../../state/store';
import { QUEUE_TITLE } from './helpers';
import { QueueModeToggle } from './QueueTray';

export interface ComposerToolbarProps {
  running: boolean;
  stopping: boolean;
  canType: boolean;
  canSend: boolean;
  canQueue: boolean;
  canCommand: boolean;
  hasContent: boolean;
  slashCommand: CommandSpec | null;
  queueMode: QueuedKind;
  onQueueModeChange: (next: QueuedKind) => void;
  sendChordLong: string;
  attachTitle: string;
  fileInput: RefObject<HTMLInputElement | null>;
  contextControl: JSX.Element | null;
  characterCounter: JSX.Element | null;
  onStop: () => void;
}

export function ComposerToolbar({
  running,
  stopping,
  canType,
  canSend,
  canQueue,
  canCommand,
  hasContent,
  slashCommand,
  queueMode,
  onQueueModeChange,
  sendChordLong,
  attachTitle,
  fileInput,
  contextControl,
  characterCounter,
  onStop,
}: ComposerToolbarProps): JSX.Element {
  return (
    <div class={`compose-bottom${running ? ' is-running' : ''}`}>
      <div class="compose-configuration">
        <div class="composer-controls compose-bar-left">
          {isUiHidden(store.pluginUi.value, UI_HIDE_COMPOSER_ATTACH) ? null : (
            <button
              type="button"
              class="compose-plus"
              onClick={() => fileInput.current?.click()}
              disabled={!canType}
              aria-label="Attach files"
              title={attachTitle}
            >
              <PaperclipIcon size={15} />
            </button>
          )}
          <div
            class="composer-session-pins"
            role="group"
            aria-label="Model and reasoning pins for subsequent runs in this session"
            title="Model and reasoning changes apply to subsequent runs, not a run already in flight."
          >
            {isUiHidden(store.pluginUi.value, UI_HIDE_COMPOSER_MODEL) ? null : <ModelControls />}
          </div>
          {/* Controls plugins contributed to this row. Nothing is drawn when
           * no plugin asked for a place here, which is the normal case. */}
          <PluginSlot slot={UI_SLOT_COMPOSER_TOOLBAR} />
        </div>
        <div class="composer-spacer" />
        {running ? null : contextControl}
        {running ? null : characterCounter}
      </div>

      {running ? (
        <>
          <div class="compose-run-actions">
            {contextControl}
            {characterCounter}
            {/* Stop ends the run; this group submits into its explicit queue.
             * Both stay visible with an empty draft. The mode choice appears
             * only once there is a message for it to describe. */}
            {hasContent && slashCommand === null ? (
              <QueueModeToggle mode={queueMode} onChange={onQueueModeChange} />
            ) : null}
            <button
              type="submit"
              class="button button-primary button-queue"
              disabled={!canQueue && !canCommand}
              aria-label={
                slashCommand !== null
                  ? `Run /${slashCommand.name}`
                  : queueMode === 'steer'
                    ? `Queue this message and steer the running turn (${sendChordLong})`
                    : `Queue this follow-up for the next turn (${sendChordLong})`
              }
              title={slashCommand !== null ? `Run /${slashCommand.name}` : QUEUE_TITLE[queueMode]}
            >
              <ArrowUpIcon size={13} />
              {slashCommand !== null ? 'Run' : queueMode === 'steer' ? 'Steer' : 'Queue'}
            </button>
          </div>
          <button
            type="button"
            class="button button-stop"
            onClick={onStop}
            disabled={stopping}
            aria-busy={stopping}
            aria-label={stopping ? 'Stopping… (Escape)' : 'Stop run (Escape)'}
            title={stopping ? 'Stopping…' : 'Stop (Escape)'}
          >
            <StopIcon size={13} />
          </button>
        </>
      ) : (
        <button
          type="submit"
          class="button button-primary button-send"
          disabled={!canSend && !canCommand}
          aria-label={
            slashCommand !== null
              ? `Run /${slashCommand.name} (${sendChordLong})`
              : `Send message (${sendChordLong})`
          }
          title={
            slashCommand !== null
              ? `Run /${slashCommand.name} (${sendChordLong})`
              : `Send (${sendChordLong})`
          }
        >
          <ArrowUpIcon size={14} />
        </button>
      )}
    </div>
  );
}
