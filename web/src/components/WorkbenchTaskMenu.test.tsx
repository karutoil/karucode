import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { resetAppState } from '../test/fixtures';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { controlPlaneOpen } from '../state/controlPlane';
import * as workbench from '../state/workbench';
import { WorkbenchTaskMenu } from './WorkbenchTaskMenu';

describe('WorkbenchTaskMenu', () => {
  beforeEach(() => {
    resetAppState();
  });

  it('opens once, retains global actions and makes Copy availability explicit', () => {
    render(<WorkbenchTaskMenu canCopy={false} onCopy={vi.fn()} />);
    const trigger = screen.getByRole('button', { name: 'Task actions' });
    expect(screen.queryByRole('menu')).toBeNull();
    fireEvent.click(trigger);

    expect(screen.getByRole('menu', { name: 'Task and workspace actions' })).not.toBeNull();
    expect(screen.getByRole('menuitem', { name: 'Copy conversation' })).toHaveProperty('disabled', true);
    expect(screen.getByRole('menuitem', { name: 'Session context' })).not.toBeNull();
    expect(screen.getByRole('menuitem', { name: /^Commands/ })).not.toBeNull();
    expect(screen.getByRole('menuitem', { name: 'Theme studio' })).not.toBeNull();
    expect(screen.getByRole('menuitem', { name: 'Settings' })).not.toBeNull();
    expect(screen.getByRole('menuitem', { name: 'All tasks' })).not.toBeNull();
  });

  it('opens the real transient Context and restores a stable trigger', () => {
    render(<WorkbenchTaskMenu canCopy onCopy={vi.fn()} />);
    const trigger = screen.getByRole('button', { name: 'Task actions' });
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Session context' }));

    expect(workbench.contextOpen.value).toBe(true);
    expect(screen.queryByRole('menu')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it('supports arrow/Home/Ends and consumes Escape without stopping a run', () => {
    const stop = vi.spyOn(actions, 'stopRun').mockImplementation(async () => undefined);
    render(<WorkbenchTaskMenu canCopy onCopy={vi.fn()} />);
    const trigger = screen.getByRole('button', { name: 'Task actions' });
    fireEvent.click(trigger);

    expect(document.activeElement?.textContent).toContain('Copy conversation');
    const menu = screen.getByRole('menu');
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    expect(document.activeElement?.textContent).toContain('Session context');
    fireEvent.keyDown(menu, { key: 'End' });
    expect(document.activeElement?.textContent).toContain('All tasks');
    fireEvent.keyDown(menu, { key: 'Home' });
    expect(document.activeElement?.textContent).toContain('Copy conversation');
    fireEvent.keyDown(menu, { key: 'Escape' });

    expect(screen.queryByRole('menu')).toBeNull();
    expect(document.activeElement).toBe(trigger);
    expect(stop).not.toHaveBeenCalled();
  });

  it.each([
    { key: 'Tab', shiftKey: false },
    { key: 'Tab', shiftKey: true },
  ])('dismisses on $key with shift=$shiftKey without trapping focus', ({ shiftKey }) => {
    render(<WorkbenchTaskMenu canCopy onCopy={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Task actions' }));
    fireEvent.keyDown(screen.getByRole('menuitem', { name: 'Session context' }), { key: 'Tab', shiftKey });

    expect(screen.queryByRole('menu')).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Task actions' }));
    fireEvent.click(document.activeElement as HTMLButtonElement);
    expect(screen.getAllByRole('menuitem').filter((item) => item.tabIndex === 0)).toHaveLength(1);
  });

  it('does not expose a project-only Context entry in a control-plane conversation', () => {
    render(<WorkbenchTaskMenu controlPlane canCopy onCopy={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Task actions' }));
    expect(screen.queryByRole('menuitem', { name: 'Session context' })).toBeNull();
    expect(screen.getByRole('menuitem', { name: 'Theme studio' })).not.toBeNull();
  });

  it('opens the existing palette and Control Plane through real actions', () => {
    render(<WorkbenchTaskMenu canCopy onCopy={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Task actions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: /^Commands/ }));
    expect(workbench.contextOpen.value).toBe(false);
    // The component proves the real global route, not a mock dialog body.
    expect(store.paletteOpen.value).toBe(true);

    fireEvent.click(screen.getByRole('button', { name: 'Task actions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'All tasks' }));
    expect(controlPlaneOpen.value).toBe(true);
    controlPlaneOpen.value = false;
  });
});
