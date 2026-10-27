import type { JSX } from 'preact';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { PanelIcon, PlusIcon, SettingsIcon, SidebarIcon } from './icons';

export function MobileToolbar(): JSX.Element {
  const sheet = store.mobileSheet.value;
  return (
    <nav class="mobile-toolbar" aria-label="Workspace panels">
      <button
        type="button"
        class={`mobile-tab${sheet === 'sessions' ? ' is-active' : ''}`}
        aria-pressed={sheet === 'sessions'}
        onClick={actions.toggleRail}
      >
        <SidebarIcon size={18} />
        <span>Sessions</span>
      </button>
      <button
        type="button"
        class="mobile-tab"
        aria-label="New session"
        onClick={() => void actions.createSession()}
      >
        <PlusIcon size={18} />
        <span>New</span>
      </button>
      <button
        type="button"
        class={`mobile-tab${sheet === 'inspector' ? ' is-active' : ''}`}
        aria-pressed={sheet === 'inspector'}
        onClick={actions.toggleInspector}
      >
        <PanelIcon size={18} />
        <span>Inspector</span>
      </button>
      <button
        type="button"
        class="mobile-tab"
        aria-haspopup="dialog"
        onClick={actions.openSettings}
      >
        <SettingsIcon size={18} />
        <span>Settings</span>
      </button>
    </nav>
  );
}