import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { emptyChatState } from './chat';
import * as store from './store';
import * as workbench from './workbench';

function resetTransientContext(): void {
  workbench.contextOpen.value = false;
  workbench.contextSection.value = 'now';
  store.narrowViewport.value = false;
  store.mobileSheet.value = null;
  store.inspectorCollapsed.value = false;
  store.inspectorTab.value = 'workspace';
  store.chat.value = emptyChatState(null);
  // Consume any snapshot left by a failed test before the next one begins.
  workbench.closeContext();
  workbench.contextOpen.value = false;
  workbench.contextSection.value = 'now';
  store.mobileSheet.value = null;
}

describe('Workbench transient context state', () => {
  beforeEach(resetTransientContext);
  afterEach(resetTransientContext);

  it('opens a requested section without touching result or chat state on desktop', () => {
    store.inspectorCollapsed.value = true;
    store.inspectorTab.value = 'changes';
    store.mobileSheet.value = 'sessions';
    const chat = emptyChatState('s_1');
    chat.running = true;
    store.chat.value = chat;

    workbench.openContext('access');

    expect(workbench.contextOpen.value).toBe(true);
    expect(workbench.contextSection.value).toBe('access');
    expect(store.inspectorCollapsed.value).toBe(true);
    expect(store.inspectorTab.value).toBe('changes');
    expect(store.mobileSheet.value).toBe('sessions');
    expect(store.chat.value).toBe(chat);

    workbench.closeContext();
    expect(workbench.contextOpen.value).toBe(false);
    expect(workbench.contextSection.value).toBe('access');
    expect(store.inspectorCollapsed.value).toBe(true);
    expect(store.inspectorTab.value).toBe('changes');
    expect(store.mobileSheet.value).toBe('sessions');
    expect(store.chat.value).toBe(chat);
  });

  it('uses the inspector sheet on phones and restores an underlying result sheet', () => {
    store.narrowViewport.value = true;
    store.mobileSheet.value = 'inspector';
    store.inspectorTab.value = 'agents';

    workbench.openContext();
    expect(store.mobileSheet.value).toBe('inspector');
    expect(workbench.contextSection.value).toBe('now');

    workbench.closeContext();
    expect(store.mobileSheet.value).toBe('inspector');
    expect(store.inspectorTab.value).toBe('agents');
  });

  it.each([null, 'sessions'] as const)('restores %s instead of leaking a new result sheet', (previous) => {
    store.narrowViewport.value = true;
    store.mobileSheet.value = previous;

    workbench.openContext('usage');
    expect(store.mobileSheet.value).toBe('inspector');
    workbench.selectContextSection('access');
    workbench.closeContext();

    expect(workbench.contextOpen.value).toBe(false);
    expect(store.mobileSheet.value).toBe(previous);
  });

  it('keeps one original phone snapshot across section opens and toggles', () => {
    store.narrowViewport.value = true;
    store.mobileSheet.value = null;
    workbench.openContext('now');
    workbench.openContext('usage');
    workbench.selectContextSection('access');
    workbench.closeContext();
    expect(store.mobileSheet.value).toBeNull();

    store.mobileSheet.value = null;
    workbench.contextSection.value = 'usage';
    workbench.toggleContext();
    expect(workbench.contextOpen.value).toBe(true);
    expect(workbench.contextSection.value).toBe('usage');
    workbench.toggleContext();
    expect(workbench.contextOpen.value).toBe(false);
    expect(store.mobileSheet.value).toBeNull();
  });

  it('does not let an old snapshot overwrite a newer external sheet interaction', () => {
    store.narrowViewport.value = true;
    store.mobileSheet.value = null;
    workbench.openContext();
    store.mobileSheet.value = 'sessions';

    workbench.closeContext();
    expect(store.mobileSheet.value).toBe('sessions');
  });

  it('selectContextSection does not implicitly open or select a result', () => {
    workbench.selectContextSection('usage');
    expect(workbench.contextOpen.value).toBe(false);
    expect(workbench.contextSection.value).toBe('usage');
    expect(store.inspectorTab.value).toBe('workspace');
  });
});
