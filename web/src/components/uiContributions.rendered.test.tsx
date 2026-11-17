import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen } from '@testing-library/preact';
import { configFixture, NOW, resetAppState } from '../test/fixtures';
import * as store from '../state/store';
import * as actions from '../state/actions';
import { EmptyState } from './EmptyState';
import { ProjectRail } from './ProjectRail';
import { SessionList } from './SessionsRail';
import { UI_REPLACE_TARGETS } from './uiContributions';

/**
 * Every advertised `replace` target must actually be drawn.
 *
 * `UI_REPLACE_TARGETS` is a promise a plugin author reads in the docs: name one
 * of these and the app will say your word instead of its own. A target nothing
 * renders is a promise the app does not keep (AGENTS.md §2), and this suite is
 * the only thing that can tell the difference between "the catalogue lists it"
 * and "a component reads it".
 *
 * The payload here renames **all** the targets at once, one distinct word each,
 * and every component in the app that consumes a target renders against it. If
 * someone adds a target without wiring it, the last test - which walks the
 * whole catalogue and demands each word on screen - fails.
 */

vi.mock('../state/actions', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../state/actions')>();
  return {
    ...actual,
    switchProject: vi.fn(async () => undefined),
    createProject: vi.fn(async () => null),
    renameProjectById: vi.fn(async () => true),
    deleteProjectById: vi.fn(async () => true),
    openThemeStudio: vi.fn(),
    togglePalette: vi.fn(),
    openSettings: vi.fn(),
    startDraftSession: vi.fn(),
    fillComposer: vi.fn(),
  };
});

/** The word drawn for each target, distinct so a mix-up cannot pass. */
const RENAMED: Record<string, string> = {
  'app.name': 'Acme',
  'label.new-session': 'New thread',
  'empty.greeting': 'What are we shipping?',
};

function withReplacements(): void {
  store.config.value = {
    ...configFixture,
    plugin_ui: {
      contributions: Object.entries(RENAMED).map(([target, replace]) => ({
        plugin: 'org-branding',
        version: '1.0.0',
        kind: 'replace',
        id: `replace-${target}`,
        hosted_id: `plugin__org-branding__replace-${target}`,
        target,
        replace,
      })),
    } as never,
  };
  store.projects.value = [
    {
      id: 'p_karucode',
      name: 'KaruCode',
      path: '/home/me/karucode',
      created_at: NOW,
      updated_at: NOW,
      session_count: 0,
    },
  ];
  store.currentProjectId.value = 'p_karucode';
  store.projectIcons.value = {};
}

afterEach(() => {
  cleanup();
  resetAppState();
});

describe('every replaceable target is rendered', () => {
  it('draws app.name in the rail lockup, keeping the mark\'s dot', () => {
    resetAppState();
    withReplacements();
    const { container } = render(<ProjectRail />);
    const mark = container.querySelector('.brand-mark');
    expect(mark?.textContent).toBe('Acme.');
    // The dot is part of the lockup, not of the name: a renamed product still
    // gets the same mark rather than a bare word.
    expect(container.querySelector('.brand-mark span:last-child')?.textContent).toBe('.');
    expect(screen.getByTestId('brand-name').textContent).toBe('Acme');
    // The control plane button keeps its own accessible name; the rename is
    // decoration, not a relabel of the button's purpose.
    expect(screen.getByRole('button', { name: 'Open control plane' })).not.toBeNull();
  });

  it('draws empty.greeting as the empty-state heading', () => {
    resetAppState();
    withReplacements();
    render(<EmptyState />);
    expect(screen.getByRole('heading', { name: 'What are we shipping?' })).not.toBeNull();
    expect(screen.getByTestId('empty-title').textContent).toBe('What are we shipping?');
  });

  it('draws label.new-session in the rail empty state', () => {
    resetAppState();
    withReplacements();
    render(
      <SessionList
        sessions={[]}
        currentId={null}
        now={NOW}
        loading={false}
        workspace="/home/me/project"
        onCreate={vi.fn()}
        onSelect={vi.fn()}
        onRename={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(screen.getByRole('button', { name: /New thread/ })).not.toBeNull();
  });

  it('renders one renamed word per listed target', () => {
    resetAppState();
    withReplacements();
    // Every component that consumes a replacement, mounted from one payload.
    const { unmount } = render(<ProjectRail />);
    const railText = document.body.textContent ?? '';
    unmount();
    const empty = render(<EmptyState />);
    const greetingText = empty.container.textContent ?? '';
    empty.unmount();
    const list = render(
      <SessionList
        sessions={[]}
        currentId={null}
        now={NOW}
        loading={false}
        workspace={undefined}
        onCreate={vi.fn()}
        onSelect={vi.fn()}
        onRename={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    const listText = list.container.textContent ?? '';
    list.unmount();

    const rendered = `${railText}\n${greetingText}\n${listText}`;
    for (const target of UI_REPLACE_TARGETS) {
      const word = RENAMED[target];
      // Adding a target to the catalogue without adding its word here (and a
      // component that draws it) fails right here.
      expect(word, `no renamed word declared for \`${target}\``).toBeDefined();
      expect(rendered, `nothing renders the replacement for \`${target}\``).toContain(word as string);
    }
  });

  it('leaves the app\'s own words when no plugin asks for a rename', () => {
    resetAppState();
    store.projects.value = [];
    store.currentProjectId.value = null;
    const rail = render(<ProjectRail />);
    expect(rail.container.querySelector('.brand-mark')?.textContent).toBe('k.');
    rail.unmount();
    render(<EmptyState />);
    expect(screen.getByRole('heading', { name: 'What are we building?' })).not.toBeNull();
  });
});

// `actions` is imported only so the module mock above replaces the real one; the
// components under test call into it for unrelated clicks.
void actions;
