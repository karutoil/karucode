import type { JSX } from 'preact';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { Button, Skeleton, Text } from '../ui';
import { AlertIcon, RefreshIcon } from './icons';

export function BootSkeleton(): JSX.Element {
  return (
    <div class="boot-skeleton" aria-busy="true" aria-label="Loading karucode workspace">
      <aside class="boot-rail">
        <Skeleton class="skeleton-brand" />
        {[0, 1, 2, 3, 4, 5].map((index) => (
          <Skeleton class="skeleton-session" key={index} />
        ))}
      </aside>
      <main class="boot-chat">
        <div class="skeleton-topbar" />
        <div class="boot-chat-body">
          <Skeleton class="skeleton-turn" />
          <Skeleton class="skeleton-turn skeleton-short" />
          <Skeleton class="skeleton-turn" />
          <Skeleton class="skeleton-composer" />
        </div>
      </main>
      <aside class="boot-inspector">
        <div class="skeleton-topbar" />
        <Skeleton class="skeleton-tree-row" />
        <Skeleton class="skeleton-tree-row" />
        <Skeleton class="skeleton-tree-row" />
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
        <Text as="h1" size="display" weight="semibold" class="state-title">
          <AlertIcon size={18} /> Could not reach karucode
        </Text>
        <Text as="p" size="message" tone="secondary" class="state-body">
          {detail}
        </Text>
        <Text as="p" size="tiny" tone="muted" class="state-footnote">
          Check that the karucode server is running (default <code>127.0.0.1:8787</code>) and that the dev proxy
          target matches <code>KARUCODE_PORT</code>.
        </Text>
        <Button
          variant="primary"
          icon={<RefreshIcon size={14} />}
          class="mt-8"
          onClick={() => void actions.bootstrap(true)}
        >
          Retry
        </Button>
      </div>
    </div>
  );
}