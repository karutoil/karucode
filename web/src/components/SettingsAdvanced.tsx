/**
 * A collapsible "Advanced" block for one settings tab.
 *
 * The sheet is split into tabs so the common settings fit on one screen; the
 * tuning knobs that most installs never touch (raw limits, timeouts, sampling
 * details) would otherwise refill that screen, so each tab can fold them into a
 * single disclosure at the bottom.
 *
 * It is a native `<details>`: the browser already hides collapsed content from
 * pointer and keyboard, and `summary` announces its own expanded state. The
 * `open` attribute is controlled instead of left to the element because a save
 * can fail on a field inside the block, and the sheet has to open it to show the
 * error. Clicking the summary is therefore prevented and replayed through
 * `onOpenChange`; Enter and Space on a summary dispatch a click, so keyboard
 * users take the same path.
 */

import type { ComponentChildren, JSX } from 'preact';
import { ChevronDownIcon } from './icons';

export interface SettingsAdvancedProps {
  /** Id of the body, so a caller can point `aria-controls` or a test at it. */
  id: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** What is in here, in one line: "Raw limits, timeouts and sampling details". */
  note: string;
  children: ComponentChildren;
}

export function SettingsAdvanced({
  id,
  open,
  onOpenChange,
  note,
  children,
}: SettingsAdvancedProps): JSX.Element {
  return (
    <details class="settings-advanced" open={open}>
      <summary
        // Replayed through state: the sheet must be able to force this open, and
        // a native toggle would leave the attribute out of sync with it.
        onClick={(event) => {
          event.preventDefault();
          onOpenChange(!open);
        }}
      >
        <span class="settings-advanced-label">
          <ChevronDownIcon size={14} />
          Advanced
        </span>
        <span class="settings-advanced-note">{note}</span>
      </summary>
      <div class="settings-advanced-body" id={id}>
        {children}
      </div>
    </details>
  );
}
