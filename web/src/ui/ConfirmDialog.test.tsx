import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { ConfirmDialog } from './ConfirmDialog';

afterEach(() => {
  document.body.innerHTML = '';
});

describe('ConfirmDialog', () => {
  it('renders nothing when closed', () => {
    render(
      <ConfirmDialog
        open={false}
        title="Delete branch"
        onClose={() => undefined}
        onConfirm={() => undefined}
      />,
    );

    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('renders a compact Overlay with title, description, body, footerNote, and Button actions', () => {
    const onClose = vi.fn();
    const onConfirm = vi.fn();
    render(
      <ConfirmDialog
        open
        title="Delete branch"
        description="This removes the local branch."
        footerNote="Cannot be undone."
        testId="branch-delete"
        onClose={onClose}
        onConfirm={onConfirm}
      >
        <p>Extra details here.</p>
      </ConfirmDialog>,
    );

    const dialog = screen.getByRole('dialog', { name: 'Delete branch' });
    expect(dialog.getAttribute('data-role')).toBe('compact');
    expect(dialog.classList.contains('overlay-compact')).toBe(true);
    expect(screen.getByText('This removes the local branch.')).not.toBeNull();
    expect(screen.getByText('Extra details here.')).not.toBeNull();
    expect(screen.getByText('Cannot be undone.')).not.toBeNull();

    const cancelBtn = screen.getByTestId('branch-delete-cancel');
    const confirmBtn = screen.getByTestId('branch-delete-confirm');
    expect(cancelBtn.textContent).toBe('Cancel');
    expect(cancelBtn.getAttribute('data-variant')).toBe('secondary');
    expect(confirmBtn.textContent).toBe('Confirm');
    expect(confirmBtn.getAttribute('data-variant')).toBe('primary');

    fireEvent.click(confirmBtn);
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();

    fireEvent.click(cancelBtn);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('supports danger tone and custom button labels', () => {
    render(
      <ConfirmDialog
        open
        title="Discard changes"
        tone="danger"
        confirmLabel="Discard"
        cancelLabel="Keep editing"
        testId="discard"
        onClose={() => undefined}
        onConfirm={() => undefined}
      />,
    );

    const confirmBtn = screen.getByTestId('discard-confirm');
    const cancelBtn = screen.getByTestId('discard-cancel');
    expect(confirmBtn.textContent).toBe('Discard');
    expect(confirmBtn.getAttribute('data-variant')).toBe('danger');
    expect(cancelBtn.textContent).toBe('Keep editing');
  });

  it('closes on Escape and backdrop click when not busy', () => {
    const onClose = vi.fn();
    render(
      <ConfirmDialog
        open
        title="Reset session"
        onClose={onClose}
        onConfirm={() => undefined}
      />,
    );

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);

    const backdrop = document.querySelector('.dialog-backdrop');
    expect(backdrop).not.toBeNull();
    fireEvent.click(backdrop as Element);
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it('locks confirm, cancel, Escape, and backdrop while busy', () => {
    const onClose = vi.fn();
    const onConfirm = vi.fn();
    render(
      <ConfirmDialog
        open
        title="Deleting server"
        busy
        testId="busy-dialog"
        onClose={onClose}
        onConfirm={onConfirm}
      />,
    );

    const confirmBtn = screen.getByTestId('busy-dialog-confirm') as HTMLButtonElement;
    const cancelBtn = screen.getByTestId('busy-dialog-cancel') as HTMLButtonElement;

    expect(confirmBtn.disabled).toBe(true);
    expect(confirmBtn.getAttribute('aria-busy')).toBe('true');
    expect(confirmBtn.querySelector('svg.animate-spin')).not.toBeNull();
    expect(cancelBtn.disabled).toBe(true);

    fireEvent.click(confirmBtn);
    fireEvent.click(cancelBtn);
    fireEvent.keyDown(document, { key: 'Escape' });
    fireEvent.click(document.querySelector('.dialog-backdrop') as Element);
    fireEvent.click(screen.getByRole('button', { name: 'Close dialog' }));

    expect(onConfirm).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });
});
