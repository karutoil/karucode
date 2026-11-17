import { describe, expect, it } from 'vitest';
import {
  applyUiPreferences,
  isUiHidden,
  matchUiKeybinding,
  matchesUiKeysOn,
  MAX_UI_CONTRIBUTIONS,
  normaliseUiKeys,
  parsePluginUi,
  pluginText,
  settingsEscapeHatch,
  settingsHides,
  slotContributions,
  UI_HIDE_COMMAND_PALETTE,
  UI_HIDE_COMPOSER_ATTACH,
  UI_HIDE_INSPECTOR_TAB_AGENTS,
  UI_HIDE_NOTIFICATIONS,
  UI_HIDE_RAIL_FOOTER,
  UI_REPLACE_GREETING,
  UI_REPLACE_NEW_SESSION_LABEL,
  UI_REPLACE_PROJECT_LABEL,
  UI_REPLACE_TARGETS,
  UI_SLOT_COMPOSER_TOOLBAR,
  UI_SLOT_INSPECTOR_HEADER,
  UI_SLOT_SESSION_ROW_BADGE,
  uiHiddenBy,
  visibleTabs,
} from './uiContributions';

/**
 * The client half of the UI-contribution contract.
 *
 * The host validates the manifest at handshake time; this suite is about the
 * second check — the one that assumes the payload could be stale, truncated or
 * written by something that is not this build. Everything here is about what is
 * *not* drawn: a wrong slot, a wrong id, a control with two behaviours.
 */

function button(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    plugin: 'kc-todo',
    version: '0.2.0',
    kind: 'button',
    id: 'add',
    hosted_id: 'plugin__kc-todo__add',
    slot: UI_SLOT_COMPOSER_TOOLBAR,
    label: 'New task',
    action: 'add',
    ...overrides,
  };
}

function command(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    plugin: 'kc-todo',
    version: '0.2.0',
    kind: 'command',
    id: 'ask',
    hosted_id: 'plugin__kc-todo__ask',
    label: 'Ask about the plan',
    message: 'Summarise the plan.',
    ...overrides,
  };
}

describe('parsing plugin_ui', () => {
  it('is empty when the server predates the field', () => {
    for (const absent of [undefined, null, 'nonsense', [], 42]) {
      const ui = parsePluginUi(absent);
      expect(ui.contributions).toEqual([]);
      expect(ui.commands).toEqual([]);
      expect(ui.hidden).toEqual({});
      expect(slotContributions(ui, UI_SLOT_COMPOSER_TOOLBAR)).toEqual([]);
    }
  });

  it('groups buttons by slot and commands separately', () => {
    const ui = parsePluginUi({
      contributions: [
        button(),
        command(),
        button({ id: 'check', hosted_id: 'plugin__kc-todo__check', slot: UI_SLOT_INSPECTOR_HEADER }),
      ],
    });
    expect(ui.refused).toEqual([]);
    expect(slotContributions(ui, UI_SLOT_COMPOSER_TOOLBAR).map((c) => c.id)).toEqual(['add']);
    expect(slotContributions(ui, UI_SLOT_INSPECTOR_HEADER).map((c) => c.id)).toEqual(['check']);
    expect(ui.commands.map((c) => c.id)).toEqual(['ask']);
    expect(ui.commands[0]?.message).toBe('Summarise the plan.');
    expect(ui.commands[0]?.slot).toBeNull();
  });

  it('refuses a control it cannot place or identify', () => {
    const cases: Array<[Record<string, unknown>, string]> = [
      [{ kind: 'webview' }, 'kind'],
      [{ id: 'add', hosted_id: 'plugin__other__add' }, 'does not match'],
      [{ id: 'Bad Id', hosted_id: 'plugin__kc-todo__Bad Id' }, 'usable id'],
      [{ label: '  ' }, 'no label'],
      [{ label: 'x'.repeat(200) }, 'character label'],
      [{ slot: 'composer.middle' }, 'does not mount'],
      [{ action: 'add', message: 'hello' }, 'both'],
      [{ action: null, message: null }, 'neither'],
      [{ action: 'Not An Action' }, 'action id'],
    ];
    for (const [override, reason] of cases) {
      const ui = parsePluginUi({ contributions: [button(override)] });
      expect(ui.contributions, JSON.stringify(override)).toEqual([]);
      expect(ui.refused.join(' '), JSON.stringify(override)).toContain(reason);
    }
  });

  it('keeps the first claim on a hosted id instead of drawing two', () => {
    const ui = parsePluginUi({
      contributions: [button(), button({ label: 'Second' })],
    });
    expect(ui.contributions).toHaveLength(1);
    expect(ui.contributions[0]?.label).toBe('New task');
    expect(ui.refused.join(' ')).toContain('declared twice');
  });

  describe('chips (text + tone)', () => {
    function chip(overrides: Record<string, unknown> = {}): Record<string, unknown> {
      return {
        plugin: 'kc-focus',
        version: '0.1.0',
        kind: 'button',
        id: 'focus',
        hosted_id: 'plugin__kc-focus__focus',
        slot: UI_SLOT_SESSION_ROW_BADGE,
        text: 'Focus',
        tone: 'accent',
        action: 'toggle',
        ...overrides,
      };
    }

    it('accepts a chip that carries only text and tone', () => {
      // No `label`: the host omits it when the plugin omitted it and the chip's
      // whole content is `text`. Refusing this was the MEDIUM-4 regression.
      const ui = parsePluginUi({ contributions: [chip()] });
      expect(ui.refused).toEqual([]);
      const [only] = slotContributions(ui, UI_SLOT_SESSION_ROW_BADGE);
      expect(only?.text).toBe('Focus');
      expect(only?.tone).toBe('accent');
      expect(only?.label).toBe('');
    });

    it('reads the tone, defaulting to neutral when it is missing or unknown', () => {
      const ui = parsePluginUi({
        contributions: [
          chip({ id: 'a', hosted_id: 'plugin__kc-focus__a', tone: 'warn' }),
          chip({ id: 'b', hosted_id: 'plugin__kc-focus__b', tone: undefined }),
          chip({ id: 'c', hosted_id: 'plugin__kc-focus__c', tone: 'chartreuse' }),
        ],
      });
      expect(ui.refused).toEqual([]);
      expect(ui.contributions.map((c) => c.tone)).toEqual(['warn', 'neutral', 'neutral']);
    });

    it('refuses a chip with neither text nor label, and over-long chip text', () => {
      const empty = parsePluginUi({ contributions: [chip({ text: '   ' })] });
      expect(empty.contributions).toEqual([]);
      expect(empty.refused.join(' ')).toContain('neither a text nor a label');

      // 24 bytes is the host's `MAX_UI_BADGE_BYTES`: inside the limit, must draw.
      const atLimit = parsePluginUi({ contributions: [chip({ text: 'x'.repeat(24) })] });
      expect(atLimit.refused).toEqual([]);
      expect(atLimit.contributions[0]?.text).toBe('x'.repeat(24));

      const tooLong = parsePluginUi({ contributions: [chip({ text: 'x'.repeat(81) })] });
      expect(tooLong.contributions).toEqual([]);
      expect(tooLong.refused.join(' ')).toContain('characters of chip text');
    });

    it('still refuses a labelled control with no label', () => {
      // The chip leniency must not reach a composer button.
      const ui = parsePluginUi({ contributions: [button({ label: '  ', text: 'Focus' })] });
      expect(ui.contributions).toEqual([]);
      expect(ui.refused.join(' ')).toContain('no label');
    });
  });

  it('bounds how many controls one payload can add', () => {
    const many = Array.from({ length: MAX_UI_CONTRIBUTIONS + 5 }, (_, index) =>
      button({
        id: `b${index}`,
        hosted_id: `plugin__kc-todo__b${index}`,
        label: `B${index}`,
        action: `a${index}`,
      }),
    );
    const ui = parsePluginUi({ contributions: many });
    expect(ui.contributions).toHaveLength(MAX_UI_CONTRIBUTIONS);
    expect(ui.refused.join(' ')).toContain('only');
  });

  it('honours only hide targets this build owns', () => {
    const ui = parsePluginUi({
      hidden: [
        { target: UI_HIDE_INSPECTOR_TAB_AGENTS, plugins: ['kc-focus'] },
        // A newer server naming something this client does not draw, or a
        // payload that tries to hide the stop button.
        { target: 'composer.stop', plugins: ['kc-focus'] },
        { target: 42, plugins: [] },
      ],
    });
    expect(isUiHidden(ui, UI_HIDE_INSPECTOR_TAB_AGENTS)).toBe(true);
    expect(uiHiddenBy(ui, UI_HIDE_INSPECTOR_TAB_AGENTS)).toEqual(['kc-focus']);
    expect(isUiHidden(ui, 'composer.stop')).toBe(false);
    expect(ui.refused.join(' ')).toContain('composer.stop');
  });

  it('carries the server catalogues through for diagnostics', () => {
    const ui = parsePluginUi({ slots: ['composer.toolbar'], hideable: ['composer.attach'] });
    expect(ui.slots).toEqual(['composer.toolbar']);
    expect(ui.hideable).toEqual(['composer.attach']);
  });
});

describe('the escape hatch back into Settings', () => {
  const hide = (targets: string[]) =>
    parsePluginUi({ hidden: targets.map((target) => ({ target, plugins: ['zen'] })) });

  it('keeps the bell when the avatar, the bell and the palette are all hidden', () => {
    const ui = hide([UI_HIDE_RAIL_FOOTER, UI_HIDE_NOTIFICATIONS, UI_HIDE_COMMAND_PALETTE]);
    const resolved = settingsHides(ui);

    expect(isUiHidden(resolved, UI_HIDE_NOTIFICATIONS)).toBe(false);
    // The avatar and the palette are still hidden: the request was honoured where
    // it was not the last way in.
    expect(isUiHidden(resolved, UI_HIDE_RAIL_FOOTER)).toBe(true);
    expect(isUiHidden(resolved, UI_HIDE_COMMAND_PALETTE)).toBe(true);
    expect(settingsEscapeHatch(ui)).toBe(UI_HIDE_NOTIFICATIONS);
  });

  it('reports no override when only the avatar was hidden, though the palette stays', () => {
    // Every hide took effect as declared; the palette is drawn because it is the
    // escape hatch once the avatar is gone, not because a hide was overridden.
    const ui = hide([UI_HIDE_RAIL_FOOTER]);
    expect(isUiHidden(settingsHides(ui), UI_HIDE_COMMAND_PALETTE)).toBe(false);
    expect(settingsEscapeHatch(ui)).toBeNull();
  });

  it('keeps the bell — not the palette — when the bell and the avatar are hidden', () => {
    const ui = hide([UI_HIDE_RAIL_FOOTER, UI_HIDE_NOTIFICATIONS]);

    expect(isUiHidden(settingsHides(ui), UI_HIDE_NOTIFICATIONS)).toBe(false);
    expect(settingsEscapeHatch(ui)).toBe(UI_HIDE_NOTIFICATIONS);
  });

  it('honours one or two hides and leaves the rest alone', () => {
    for (const targets of [
      [UI_HIDE_NOTIFICATIONS],
      [UI_HIDE_COMMAND_PALETTE],
      [UI_HIDE_NOTIFICATIONS, UI_HIDE_COMMAND_PALETTE],
    ]) {
      const ui = hide(targets);
      expect(settingsHides(ui)).toBe(ui);
      expect(settingsEscapeHatch(ui)).toBeNull();
    }
  });

  it('keeps the bell whenever the bell was asked for alongside the avatar', () => {
    // Avatar + palette: the bell must stay, or nothing would open Settings.
    const withPalette = hide([UI_HIDE_RAIL_FOOTER, UI_HIDE_COMMAND_PALETTE]);
    expect(isUiHidden(settingsHides(withPalette), UI_HIDE_NOTIFICATIONS)).toBe(false);

    // Avatar + bell: the palette stays, because the bell cannot open Settings.
    const withBell = hide([UI_HIDE_RAIL_FOOTER, UI_HIDE_NOTIFICATIONS]);
    expect(isUiHidden(settingsHides(withBell), UI_HIDE_COMMAND_PALETTE)).toBe(false);
  });

  it('always leaves Settings reachable, from any starting state', () => {
    const all = [UI_HIDE_RAIL_FOOTER, UI_HIDE_NOTIFICATIONS, UI_HIDE_COMMAND_PALETTE];
    for (let mask = 0; mask < 1 << all.length; mask += 1) {
      const targets = all.filter((_, index) => (mask & (1 << index)) !== 0);
      const resolved = settingsHides(hide(targets));
      // Directly, or through the palette's *Settings* command: an entry the
      // person can click that ends in Settings. This is the property the rule
      // exists for, asserted over every combination rather than by example.
      const reachable =
        !isUiHidden(resolved, UI_HIDE_RAIL_FOOTER) ||
        !isUiHidden(resolved, UI_HIDE_NOTIFICATIONS) ||
        !isUiHidden(resolved, UI_HIDE_COMMAND_PALETTE);
      expect({ targets, reachable }).toEqual({ targets, reachable: true });
      // A second pass keeps the same controls on screen, so the store may apply
      // the rule after the person's overrides without undoing anything.
      // A second pass over the same *request* (what the payload asked for) yields
      // the same hides, so the store may apply the rule after the person's
      // overrides without a later pass changing what is on screen.
      expect(settingsHides(hide(targets)).hidden).toEqual(resolved.hidden);
    }
  });
});

describe('hiding built-in tabs', () => {
  const tabs = [
    { id: 'workspace' },
    { id: 'changes' },
    { id: 'session' },
    { id: 'agents' },
  ];
  const target = (id: string): string | null =>
    id === 'agents' ? UI_HIDE_INSPECTOR_TAB_AGENTS : `inspector.tab.${id}`;

  it('drops a hidden tab and keeps the order of the rest', () => {
    const ui = parsePluginUi({ hidden: [{ target: UI_HIDE_INSPECTOR_TAB_AGENTS, plugins: ['p'] }] });
    expect(visibleTabs(tabs, ui, target).tabs.map((t) => t.id)).toEqual([
      'workspace',
      'changes',
      'session',
    ]);
  });

  it('never leaves the strip empty, and says the request was refused', () => {
    const ui = parsePluginUi({
      hidden: [
        { target: 'inspector.tab.workspace', plugins: ['p'] },
        { target: 'inspector.tab.changes', plugins: ['p'] },
        { target: 'inspector.tab.session', plugins: ['p'] },
        { target: UI_HIDE_INSPECTOR_TAB_AGENTS, plugins: ['p'] },
      ],
    });
    const result = visibleTabs(tabs, ui, target);
    expect(result.tabs).toHaveLength(4);
    expect(result.refused).toBe(true);
  });

  it('leaves a client-only list alone when nothing is hidden', () => {
    const ui = parsePluginUi({});
    expect(visibleTabs(tabs, ui, target)).toEqual({ tabs, refused: false });
  });
});

describe('the composer attach button', () => {
  it('is hideable by name, and nothing else about the composer is', () => {
    const ui = parsePluginUi({
      hidden: [{ target: UI_HIDE_COMPOSER_ATTACH, plugins: ['kc-focus'] }],
    });
    expect(isUiHidden(ui, UI_HIDE_COMPOSER_ATTACH)).toBe(true);
    expect(isUiHidden(ui, 'composer.send')).toBe(false);
    expect(ui.refused).toEqual([]);
  });
});

describe('contributed keyboard shortcuts', () => {
  function binding(overrides: Record<string, unknown> = {}): Record<string, unknown> {
    return {
      plugin: 'kc-focus',
      version: '1.0.0',
      kind: 'keybinding',
      id: 'focus-key',
      hosted_id: 'plugin__kc-focus__focus-key',
      label: 'Focus mode shortcut',
      keys: 'mod+shift+f',
      action: 'focus',
      ...overrides,
    };
  }

  it('normalises the spelling so one chord compares as one string', () => {
    expect(normaliseUiKeys('Mod+Shift+F')).toBe('mod+shift+f');
    expect(normaliseUiKeys('shift+mod+f')).toBe('mod+shift+f');
    expect(normaliseUiKeys('alt + g')).toBe('alt+g');
    expect(normaliseUiKeys('ctrl+enter')).toBe('ctrl+enter');
  });

  it('refuses a spec that is not a shortcut', () => {
    // A bare key is typing; shift alone is still typing; a modifier with no key
    // is a half-written spec.
    for (const bad of ['', 'f', 'shift+f', 'mod', 'mod+shift', 'mod+f+g', 'mod+f1', 'mod+']) {
      expect(normaliseUiKeys(bad), bad).toBeNull();
    }
  });

  it('accepts a binding and canonicalises its chord', () => {
    const ui = parsePluginUi({ contributions: [binding({ keys: 'Shift+Mod+F' })] });
    expect(ui.refused).toEqual([]);
    expect(ui.keybindings.map((b) => b.keys)).toEqual(['mod+shift+f']);
    expect(ui.keybindings[0]?.slot).toBeNull();
    // A binding is not a button or a command, so it never lands in a slot.
    expect(ui.contributions).toHaveLength(1);
    expect(ui.commands).toEqual([]);
  });

  it('refuses the app\u2019s own chords and a chord a second plugin already took', () => {
    const ui = parsePluginUi({
      contributions: [
        binding({ plugin: 'a', hosted_id: 'plugin__a__focus-key' }),
        binding({
          plugin: 'a',
          id: 'clash',
          hosted_id: 'plugin__a__clash',
          keys: 'mod+shift+f',
          action: 'other',
        }),
        binding({
          plugin: 'a',
          id: 'palette',
          hosted_id: 'plugin__a__palette',
          keys: 'mod+k',
          action: 'x',
        }),
      ],
    });
    expect(ui.keybindings.map((b) => b.hosted_id)).toEqual(['plugin__a__focus-key']);
    const reasons = ui.refused.join(' ');
    expect(reasons).toContain('already bound by');
    expect(reasons).toContain('one of the app');
  });

  it('matches a keyboard event on whichever modifier the platform calls mod', () => {
    const event = (init: KeyboardEventInit): KeyboardEvent =>
      ({
        key: 'f',
        metaKey: false,
        ctrlKey: false,
        altKey: false,
        shiftKey: false,
        ...init,
      }) as KeyboardEvent;
    // Everywhere but macOS, mod is Control.
    expect(matchesUiKeysOn(event({ ctrlKey: true, shiftKey: true }), 'mod+shift+f', { mac: false })).toBe(true);
    expect(matchesUiKeysOn(event({ metaKey: true, shiftKey: true }), 'mod+shift+f', { mac: false })).toBe(false);
    expect(matchesUiKeysOn(event({ ctrlKey: true }), 'mod+shift+f', { mac: false })).toBe(false);
    expect(matchesUiKeysOn(event({ ctrlKey: true, key: ' ' }), 'mod+space', { mac: false })).toBe(true);
    expect(matchesUiKeysOn(event({ ctrlKey: true, key: '/' }), 'mod+/', { mac: false })).toBe(true);

    // On macOS the same spec follows Cmd, and `ctrl` stays the literal key.
    expect(matchesUiKeysOn(event({ metaKey: true, shiftKey: true }), 'mod+shift+f', { mac: true })).toBe(true);
    expect(matchesUiKeysOn(event({ ctrlKey: true, shiftKey: true }), 'mod+shift+f', { mac: true })).toBe(false);
    expect(matchesUiKeysOn(event({ ctrlKey: true, key: 'g' }), 'ctrl+g', { mac: true })).toBe(true);
    expect(matchesUiKeysOn(event({ metaKey: true, key: 'g' }), 'ctrl+g', { mac: true })).toBe(false);
  });

  it('picks the first matching binding, so one keystroke runs one action', () => {
    const ui = parsePluginUi({
      contributions: [
        binding({ plugin: 'a', id: 'a', hosted_id: 'plugin__a__a', keys: 'mod+1', action: 'one' }),
        binding({ plugin: 'a', id: 'b', hosted_id: 'plugin__a__b', keys: 'mod+2', action: 'two' }),
      ],
    });
    // Whatever this host calls `mod`, the same chord finds its row.
    const mac = /mac|iphone|ipad/i.test(`${navigator.platform} ${navigator.userAgent}`);
    const event = (key: string): KeyboardEvent =>
      ({
        key,
        metaKey: mac,
        ctrlKey: !mac,
        altKey: false,
        shiftKey: false,
      }) as KeyboardEvent;
    expect(matchUiKeybinding(ui.keybindings, event('2'))?.action).toBe('two');
    expect(matchUiKeybinding(ui.keybindings, event('9'))).toBeNull();
  });

  it('drops a switched-off binding like any other contribution', () => {
    const ui = parsePluginUi({ contributions: [binding()] });
    const filtered = applyUiPreferences(ui, {
      disabled: ['plugin__kc-focus__focus-key'],
      shown: [],
    });
    expect(filtered.keybindings).toEqual([]);
  });

  describe('replace (renaming a built-in label)', () => {
    it('renames a replaceable target and leaves everything else alone', () => {
      const ui = parsePluginUi({
        contributions: [
          {
            plugin: 'org-branding',
            kind: 'replace',
            id: 'replace-label.new-session',
            hosted_id: 'plugin__org-branding__replace-label.new-session',
            target: UI_REPLACE_NEW_SESSION_LABEL,
            replace: 'New thread',
          },
        ],
      });
      expect(pluginText(ui, UI_REPLACE_NEW_SESSION_LABEL, 'New session')).toBe('New thread');
      // A target nobody replaced keeps the app's own word.
      expect(pluginText(ui, UI_REPLACE_GREETING, 'What are we building?')).toBe('What are we building?');
      // The replace entry is not a drawable control.
      expect(ui.contributions).toEqual([]);
    });

    it('lists exactly the three targets the app draws, and mirrors the host', () => {
      // The catalogue is a promise: every entry names a label `pluginText` is
      // called with somewhere in the app. All three are rendered by
      // `uiContributions.rendered.test.ts`, which builds one payload renaming
      // every entry at once and asserts the text reaches the screen.
      //
      // The literal is the host's `kc_core::plugin::UI_REPLACEABLE`; the two
      // lists are a contract, so a change on one side that forgets the other
      // fails here rather than in production.
      expect(UI_REPLACE_TARGETS).toEqual([
        'app.name',
        'label.new-session',
        'empty.greeting',
      ]);
      // The two label words no single control draws are reserved, not listed.
      expect(UI_REPLACE_TARGETS).not.toContain('label.session');
      expect(UI_REPLACE_TARGETS).not.toContain('label.project');
    });

    it('refuses a target outside the catalogue or an empty replacement', () => {
      const ui = parsePluginUi({
        contributions: [
          {
            plugin: 'evil',
            kind: 'replace',
            id: 'replace-approve',
            hosted_id: 'plugin__evil__replace-approve',
            target: 'approval.approve',
            replace: 'Go',
          },
          {
            plugin: 'evil',
            kind: 'replace',
            id: 'replace-label.new-session',
            hosted_id: 'plugin__evil__replace-label.new-session',
            target: UI_REPLACE_NEW_SESSION_LABEL,
            replace: '   ',
          },
        ],
      });
      expect(pluginText(ui, 'approval.approve', 'Approve')).toBe('Approve');
      expect(pluginText(ui, UI_REPLACE_NEW_SESSION_LABEL, 'New session')).toBe('New session');
      const refused = ui.refused.join(' ');
      expect(refused).toContain('approval.approve');
      expect(refused).toContain('has no text');
    });

    it('refuses the reserved label words nothing draws', () => {
      const ui = parsePluginUi({
        contributions: [
          {
            plugin: 'org-branding',
            kind: 'replace',
            id: 'replace-label.project',
            hosted_id: 'plugin__org-branding__replace-label.project',
            target: UI_REPLACE_PROJECT_LABEL,
            replace: 'Workspace',
          },
        ],
      });
      expect(pluginText(ui, UI_REPLACE_PROJECT_LABEL, 'Project')).toBe('Project');
      expect(ui.refused.join(' ')).toContain('`label.project` is not replaceable');
    });

    it('lets the first claim on a target win and says the later one was dropped', () => {
      const ui = parsePluginUi({
        contributions: [
          {
            plugin: 'a',
            kind: 'replace',
            id: 'replace-app-name',
            hosted_id: 'plugin__a__replace-app-name',
            target: 'app.name',
            replace: 'Acme',
          },
          {
            plugin: 'b',
            kind: 'replace',
            id: 'replace-app-name',
            hosted_id: 'plugin__b__replace-app-name',
            target: 'app.name',
            replace: 'Other',
          },
        ],
      });
      expect(pluginText(ui, 'app.name', 'k')).toBe('Acme');
      expect(ui.refused.join(' ')).toContain('renamed twice');
    });
  });
});
