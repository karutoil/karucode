import type { JSX } from 'preact';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { AlertIcon, RefreshIcon } from './icons';

export function BootSkeleton(): JSX.Element {
  return (
    <div class="boot-skeleton" aria-busy="true" aria-label="Loading karucode workspace">
      <aside class="boot-rail">
        <div class="skeleton skeleton-brand" />
        {[0, 1, 2, 3, 4, 5].map((index) => (
          <div class="skeleton skeleton-session" key={index} />
        ))}
      </aside>
      <main class="boot-chat">
        <div class="skeleton skeleton-topbar" />
        <div class="boot-chat-body">
          <div class="skeleton skeleton-turn" />
          <div class="skeleton skeleton-turn skeleton-short" />
          <div class="skeleton skeleton-turn" />
          <div class="skeleton skeleton-composer" />
        </div>
      </main>
      <aside class="boot-inspector">
        <div class="skeleton skeleton-topbar" />
        <div class="skeleton skeleton-tree-row" />
        <div class="skeleton skeleton-tree-row" />
        <div class="skeleton skeleton-tree-row" />
      </aside>
    </div>
  );
}

export function BootErrorScreen(): JSX.Element {
  const message = store.bootError.value ?? 'The API did not respond.';
  const detail = message.startsWith('Could not reach karucode: ')
    ? message.slice('Could not reach karucode: '.length)
    : message;
  return (
    <div class="full-screen-state">
      <div class="state-card" role="alert">
        <div class="brand brand-large">
          <span class="brand-dot" aria-hidden="true" />
          <span class="brand-name">karucode</span>
        </div>
        <h1 class="state-title">
          <AlertIcon size={18} /> Could not reach karucode
        </h1>
        <p class="state-body">{detail}</p>
        <p class="state-footnote">
          Check that the karucode server is running (default <code>127.0.0.1:8787</code>) and that the dev proxy
          target matches <code>KARUCODE_PORT</code>.
        </p>
        <button type="button" class="button button-primary" onClick={() => void actions.bootstrap(true)}>
          <RefreshIcon size={14} /> Retry
        </button>
      </div>
    </div>
  );
}