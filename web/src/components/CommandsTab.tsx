import type { JSX } from 'preact';
import * as store from '../state/store';
import { CommandCard } from './CommandCard';
import { TerminalIcon } from './icons';

export function CommandsTab(): JSX.Element {
  const commands = [...store.chat.value.items]
    .filter((item) => item.kind === 'command')
    .reverse();

  return (
    <div class="inspector-section">
      <div class="section-toolbar">
        <div class="section-heading">
          <span class="eyebrow">Commands</span>
          <span class="section-path">{commands.length} in this session</span>
        </div>
      </div>

      {commands.length === 0 ? (
        <div class="empty-inspector">
          <p class="empty-inspector-title">No commands yet</p>
          <p>
            When the agent runs a shell command, its stdout, stderr and exit code appear here and inline in the
            transcript.
          </p>
        </div>
      ) : (
        <div class="command-list">
          {commands.map((item) =>
            item.kind === 'command' ? <CommandCard key={item.id} item={item} defaultOpen /> : null,
          )}
        </div>
      )}

      <p class="inspector-note">
        <TerminalIcon size={12} /> Commands run on the server as its user; they are not sandboxed.
      </p>
    </div>
  );
}