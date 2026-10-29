import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { CommandsTab } from './CommandsTab';
import { emptyChatState, type CommandItem } from '../state/chat';
import { resetAppState } from '../test/fixtures';
import * as store from '../state/store';

function command(partial: Partial<CommandItem> & Pick<CommandItem, 'id' | 'command'>): CommandItem {
  return {
    kind: 'command',
    at: 0,
    callId: partial.id,
    cwd: '.',
    stdout: '',
    stderr: '',
    output: '',
    exitCode: 0,
    status: 'ok',
    truncated: false,
    durationMs: null,
    runId: 'r1',
    ...partial,
  };
}

/** Oldest first, as the transcript stores them; the tab shows newest first. */
function threeCommands(): CommandItem[] {
  return [
    command({ id: 'cmd:1', at: 1, command: 'cargo fmt --check', stdout: 'Clean' }),
    command({ id: 'cmd:2', at: 2, command: 'rg "PluginManifest" src/', stdout: '12 matches' }),
    command({
      id: 'cmd:3',
      at: 3,
      command: 'cargo test --workspace',
      stdout: 'test result: ok. 48 passed; 0 failed',
      durationMs: 1310,
    }),
  ];
}

function seed(items: CommandItem[]): void {
  store.chat.value = { ...emptyChatState('s_ab12'), items };
}

describe('CommandsTab', () => {
  beforeEach(() => {
    resetAppState();
    store.currentProjectId.value = null;
  });

  afterEach(() => {
    resetAppState();
  });

  it('lists the newest command first and opens only that card', () => {
    seed(threeCommands());
    const { container } = render(<CommandsTab />);

    const cards = [...container.querySelectorAll<HTMLElement>('.command-card')];
    expect(cards).toHaveLength(3);
    expect(cards.map((card) => card.querySelector('.command-text')?.textContent)).toEqual([
      'cargo test --workspace',
      'rg "PluginManifest" src/',
      'cargo fmt --check',
    ]);

    const summaries = cards.map((card) => card.querySelector<HTMLElement>('.command-summary')!);
    expect(summaries[0]?.getAttribute('aria-expanded')).toBe('true');
    expect(summaries[1]?.getAttribute('aria-expanded')).toBe('false');
    expect(summaries[2]?.getAttribute('aria-expanded')).toBe('false');

    // The open card's output is the real stdout, and the closed ones are empty.
    const outputs = cards.map((card) => card.querySelector<HTMLElement>('.cmd-output')!);
    expect(outputs[0]?.hidden).toBe(false);
    expect(outputs[0]?.textContent).toContain('48 passed');
    expect(outputs[1]?.hidden).toBe(true);
    expect(outputs[2]?.hidden).toBe(true);
  });

  it('discloses an older card on demand without closing the newest one', () => {
    seed(threeCommands());
    const { container } = render(<CommandsTab />);

    const cards = [...container.querySelectorAll<HTMLElement>('.command-card')];
    fireEvent.click(cards[2]!.querySelector<HTMLElement>('.command-summary')!);

    expect(cards[2]!.querySelector<HTMLElement>('.command-summary')?.getAttribute('aria-expanded')).toBe(
      'true',
    );
    expect(cards[2]!.querySelector<HTMLElement>('.cmd-output')?.hidden).toBe(false);
    // `defaultOpen` is additive: opening an older card never closes the newest.
    expect(cards[0]!.querySelector<HTMLElement>('.command-summary')?.getAttribute('aria-expanded')).toBe(
      'true',
    );
  });

  it('counts the executions in the kicker row', () => {
    seed(threeCommands());
    render(<CommandsTab />);
    expect(screen.getByText('Recent executions')).not.toBeNull();
    expect(screen.getByText('3 commands')).not.toBeNull();
  });

  it('explains the empty state instead of rendering an empty list', () => {
    seed([]);
    const { container } = render(<CommandsTab />);
    expect(container.querySelectorAll('.command-card')).toHaveLength(0);
    expect(screen.getByText('No commands yet')).not.toBeNull();
    expect(screen.getByText('0 commands')).not.toBeNull();
  });

  it('keeps the sandbox warning visible', () => {
    seed([]);
    render(<CommandsTab />);
    expect(screen.getByText(/Commands run on the server as its user/)).not.toBeNull();
  });
});
