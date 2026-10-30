import { render } from 'preact';
import { App } from './app';
import { ErrorBoundary } from './components/ErrorBoundary';
import { initTheme } from './theme';
// Draws the kc-todo plugin's inspector panel with a first-party component
// instead of the generic JSON fallback; imported for its side effect.
import './components/todoPanelRegister';
import './styles/index.css';

// Paint the persisted theme before the first render so the workspace never
// flashes the default palette. `styles/tokens.css` ships Ember as its fallback,
// which is what `initTheme()` reapplies when nothing is stored.
initTheme();

const root = document.getElementById('app');
if (root) {
  // A render that throws must not leave a white page: the boundary keeps the
  // error on screen with a reload button and copyable diagnostics.
  render(
    <ErrorBoundary scope="The workspace">
      <App />
    </ErrorBoundary>,
    root,
  );
}
