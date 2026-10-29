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
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">Recent executions</span>
        <span class="inspector-kicker-note">
          {commands.length} {commands.length === 1 ? 'command' : 'commands'}
        </span>
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
          {commands.map((item, index) =>
            item.kind === 'command' ? (
              <CommandCard key={item.id} item={item} defaultOpen={index === 0} />
            ) : null,
          )}
        </div>
      )}

      <p class="inspector-note">
        <TerminalIcon size={12} /> Commands run on the server as its user; they are not sandboxed.
      </p>
    </div>
  );
}