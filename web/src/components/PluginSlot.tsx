import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import * as actions from '../state/actions';
import * as store from '../state/store';
import {
  UI_BADGE_SLOTS,
  UI_TONE_DEFAULT,
  UI_TONES,
  slotContributions,
} from './uiContributions';
import { pluginPanelIcon } from './icons';

/**
 * The tone class a chip is drawn with.
 *
 * `uiContributions.parsePluginUi` has already resolved the tone to one of
 * `UI_TONES`, so this only guards against a caller building a contribution by
 * hand: an unknown tone would produce the empty class `session-chip-`, which
 * silently drops the chip's only visual signal.
 */
function chipTone(tone: string): string {
  return UI_TONES.includes(tone) ? tone : UI_TONE_DEFAULT;
}

/**
 * One plugin-contributed control, drawn by the shell.
 *
 * It is a plain `<button>`: the plugin supplied a label, a tooltip and the *name*
 * of an icon, never markup, a style or a handler. A click runs
 * `actions.runPluginContributions` (the only code that talks to the network) and
 * the control reports its own busy state so a slow action cannot be fired twice.
 *
 * Attribution is part of the design: every contributed control carries a dot and
 * a tooltip naming its plugin, because a person who cannot tell a plugin's button
 * from a built-in one is one surprising click away from a bad afternoon.
 */
function ContributionButton({
  contribution,
}: {
  contribution: store.PluginUiContribution;
}): JSX.Element {
  const [running, setRunning] = useState(false);
  const attribution = `from the ${contribution.plugin} plugin (v${contribution.version || '?'})`;
  // A chip (`session.row.badge`, `session.header.chip`) is drawn from `text` and a
  // tone, not from `label`: the host fills `label` in from `text` when the plugin
  // gave none, and it stays the accessible name either way, so a screen reader
  // hears the same words a sighted reader sees on the chip.
  const is_chip = contribution.slot !== null && UI_BADGE_SLOTS.includes(contribution.slot);
  const shown = is_chip && contribution.text !== null ? contribution.text : contribution.label;
  // The visible label is the accessible name, so a screen reader hears what the
  // button says. The tooltip adds the plugin's own explanation and, always, the
  // attribution: a control whose label is ellipsised in a narrow row still has
  // its full name one hover (and one announcement) away.
  const tooltip =
    contribution.title && contribution.title !== contribution.label
      ? `${contribution.title} — ${attribution}`
      : `${contribution.label} — ${attribution}`;

  const run = (): void => {
    if (running) return;
    setRunning(true);
    void actions.runPluginContributions(contribution).finally(() => setRunning(false));
  };

  return (
    <button
      type="button"
      class={
        is_chip
          ? `button session-chip session-chip-${chipTone(contribution.tone)} plugin-slot-button plugin-slot-chip`
          : 'button button-small plugin-slot-button'
      }
      data-plugin={contribution.plugin}
      data-contribution={contribution.id}
      data-tone={is_chip ? chipTone(contribution.tone) : undefined}
      data-running={running ? 'true' : 'false'}
      title={tooltip}
      aria-label={contribution.label}
      aria-busy={running ? 'true' : undefined}
      disabled={running}
      onClick={run}
    >
      {contribution.icon && !is_chip ? (
        <span class="plugin-slot-icon" aria-hidden="true">
          {pluginPanelIcon(contribution.icon, { size: 13 })}
        </span>
      ) : null}
      <span class="plugin-slot-label">{shown}</span>
      <span class="plugin-slot-dot" aria-hidden="true" />
    </button>
  );
}

export interface PluginSlotProps {
  /** One of `UI_SLOT_IDS` (`composer.toolbar`, `inspector.header`, …). */
  slot: string;
  /** Extra class for the wrapper, so a slot can sit in its own row. */
  class?: string;
}

/**
 * Every control plugins contributed to one slot.
 *
 * Renders nothing at all when the slot is empty — no wrapper, no gap — which is
 * why a workspace with no plugins looks exactly as it did before this existed.
 * The slot name is a closed set the app owns (see `components/uiContributions.ts`):
 * a plugin cannot mint a position, only ask for one the app already has.
 */
export function PluginSlot({ slot, class: extraClass }: PluginSlotProps): JSX.Element | null {
  const contributions = slotContributions(store.pluginUi.value, slot);
  if (contributions.length === 0) return null;
  // A badge slot holds chips, so the group says so: "Plugin chips" is what a
  // screen-reader user finds there, and labelling it "Plugin controls" would
  // promise buttons that are not there.
  const is_chip_slot = UI_BADGE_SLOTS.includes(slot);
  return (
    <div
      class={`plugin-slot${extraClass ? ` ${extraClass}` : ''}`}
      data-slot={slot}
      role="group"
      aria-label={is_chip_slot ? 'Plugin chips' : 'Plugin controls'}
    >
      {contributions.map((contribution) => (
        <ContributionButton key={contribution.hosted_id} contribution={contribution} />
      ))}
    </div>
  );
}
