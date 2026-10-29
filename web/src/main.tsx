import { render } from 'preact';
import { App } from './app';
import { initTheme } from './theme';
import './styles/index.css';

// Paint the persisted theme before the first render so the workspace never
// flashes the default palette. `styles/tokens.css` ships Ember as its fallback,
// which is what `initTheme()` reapplies when nothing is stored.
initTheme();

const root = document.getElementById('app');
if (root) render(<App />, root);