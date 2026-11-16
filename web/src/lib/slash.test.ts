import { describe, expect, it } from 'vitest';
import type { CommandSpec } from '../api/types';
import {
  commandArgs,
  commandName,
  insertCommand,
  isCommandToken,
  knownCommand,
  rankCommands,
  scoreCommand,
  slashQuery,
  SLASH_MENU_LIMIT,
} from './slash';

function command(
  name: string,
  overrides: Partial<CommandSpec> = {},
): CommandSpec {
  return {
    name,
    title: `${name} title`,
    summary: `${name} summary`,
    args: '',
    category: 'session',
    source: 'builtin',
    insert: `/${name}`,
    ...overrides,
  };
}

const CATALOG: CommandSpec[] = [
  command('help', { category: 'meta', summary: 'What each command does' }),
  command('compact', { category: 'context', summary: 'Fold older turns into a digest' }),
  command('model', { args: '[provider/model]', insert: '/model ', summary: 'Pin the model' }),
  command('skill', { args: '<name> [task]', insert: '/skill ', category: 'extend' }),
  command('release', { category: 'extend', source: 'skill', insert: '/release ' }),
];

describe('slash command parsing', () => {
  it('reads a command as the whole first token', () => {
    expect(commandName('/compact')).toBe('compact');
    expect(commandName('/HELP')).toBe('help');
    expect(commandName('/model gpt-4.1')).toBe('model');
    expect(commandArgs('/model gpt-4.1')).toBe('gpt-4.1');
    expect(commandArgs('/rename  two  spaces ')).toBe('two  spaces');
  });

  it('leaves anything that only starts with a slash as a message', () => {
    // The path case is why the rule exists at all.
    expect(commandName('/home/toil/notes.md — why?')).toBeNull();
    expect(commandName('// a comment')).toBeNull();
    expect(commandName('/')).toBeNull();
    expect(commandName(' /compact')).toBeNull();
    expect(commandName('run /compact')).toBeNull();
    expect(commandName('/compact!')).toBeNull();
    expect(commandName('/9lives')).toBeNull();
    expect(commandName('compact')).toBeNull();
  });

  it('accepts only lowercase command tokens as skill names', () => {
    expect(isCommandToken('release')).toBe(true);
    expect(isCommandToken('fix-bug_2')).toBe(true);
    expect(isCommandToken('Release')).toBe(false);
    expect(isCommandToken('fix.bug')).toBe(false);
    expect(isCommandToken('')).toBe(false);
  });
});

describe('the slash menu query', () => {
  it('is the typed name while the whole draft is the command', () => {
    expect(slashQuery('/', 1)).toBe('');
    expect(slashQuery('/com', 4)).toBe('com');
    expect(slashQuery('/COM', 4)).toBe('com');
  });

  it('closes as soon as arguments start or the caret moves back', () => {
    expect(slashQuery('/model gpt', 10)).toBeNull();
    expect(slashQuery('/compact', 3)).toBeNull();
    expect(slashQuery('hello /', 7)).toBeNull();
    expect(slashQuery('', 0)).toBeNull();
  });
});

describe('ranking the catalog', () => {
  it('keeps the server order for an empty query', () => {
    expect(rankCommands(CATALOG, '').map((entry) => entry.name)).toEqual([
      'help',
      'compact',
      'model',
      'skill',
      'release',
    ]);
  });

  it('puts the exact name first, then prefixes, then descriptions', () => {
    expect(rankCommands(CATALOG, 'model')[0]?.name).toBe('model');
    // A name prefix outranks a word that only appears in another row's summary.
    const ranked = rankCommands(CATALOG, 'co').map((entry) => entry.name);
    expect(ranked[0]).toBe('compact');
    expect(ranked).toContain('help');
    // The word only appears in a summary of one command.
    expect(rankCommands(CATALOG, 'digest').map((entry) => entry.name)).toEqual(['compact']);
  });

  it('drops what does not match at all', () => {
    expect(rankCommands(CATALOG, 'zzz')).toEqual([]);
    expect(scoreCommand(CATALOG[0]!, 'zzz')).toBe(0);
  });

  it('honours the limit', () => {
    const many = Array.from({ length: SLASH_MENU_LIMIT + 4 }, (_, index) => command(`c${index}`));
    expect(rankCommands(many, '')).toHaveLength(SLASH_MENU_LIMIT);
    expect(rankCommands(many, '', 2)).toHaveLength(2);
  });
});

describe('recognising a whole command', () => {
  it('finds the catalog row for a draft that is exactly a command', () => {
    expect(knownCommand(CATALOG, '/model gpt-4.1')?.name).toBe('model');
    expect(knownCommand(CATALOG, '/nope')).toBeNull();
    expect(knownCommand(CATALOG, 'plain text')).toBeNull();
    // The catalog is only what this project offered: an entry it lacks is not a
    // command here, and the server decides again anyway.
    expect(knownCommand([], '/help')).toBeNull();
  });

  it('inserts the usage the server sent, trailing space included', () => {
    expect(insertCommand(command('compact'))).toBe('/compact');
    expect(insertCommand(command('model', { args: '[id]', insert: '/model ' }))).toBe('/model ');
    // A row that forgot its `insert` still yields something typeable.
    expect(insertCommand(command('help', { insert: '' }))).toBe('/help');
  });
});