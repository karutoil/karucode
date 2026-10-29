import type { JSX } from 'preact';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { PanelIcon, PlusIcon, SettingsIcon, SidebarIcon } from './icons';

/**
 * Bottom bar for narrow viewports: the rails become sheets here, so this is the
 * only always-visible way to open the session list and the inspector.
 */
export function MobileToolbar(): JSX.Element {
  const sheet = store.mobileSheet.value;
  return (
    <nav class="mobile-toolbar" aria-label="Workspace panels">
      <button
        type="button"
        class={`mobile-tab${sheet === 'sessions' ? ' is-active' : ''}`}
        aria-pressed={sheet === 'sessions'}
        aria-label="Sessions"
        aria-keyshortcuts="Control+B Meta+B"
        onClick={actions.toggleRail}
      >
        <SidebarIcon size={18} />
        <span>Sessions</span>
      </button>
      <button
        type="button"
        class="mobile-tab"
        aria-label="New session"
        aria-keyshortcuts="Control+N Meta+N"
        onClick={() => void actions.createSession()}
      >
        <PlusIcon size={18} />
        <span>New</span>
      </button>
      <button
        type="button"
        class={`mobile-tab${sheet === 'inspector' ? ' is-active' : ''}`}
        aria-pressed={sheet === 'inspector'}
        aria-label="Inspector"
        aria-keyshortcuts="Control+J Meta+J"
        onClick={actions.toggleInspector}
      >
        <PanelIcon size={18} />
        <span>Inspector</span>
      </button>
      <button
        type="button"
        class="mobile-tab"
        aria-haspopup="dialog"
        aria-label="Settings"
        onClick={actions.openSettings}
      >
        <SettingsIcon size={18} />
        <span>Settings</span>
      </button>
    </nav>
  );
}