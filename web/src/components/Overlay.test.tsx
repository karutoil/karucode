import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { useState } from 'preact/hooks';
import { Overlay, type OverlayRole } from './Overlay';
import { Dialog } from './Dialog';
import { resetAppState } from '../test/fixtures';

/**
 * The shell's own contract.
 *
 * Every modal in the product is this component, so the facts pinned here — one
 * `aria-modal` surface, a portal into `<body>`, a named landmark, a body that
 * owns the scroll, a footer that never scrolls, the one close button, Escape
 * and the backdrop — are pinned once instead of being re-discovered through
 * thirteen surface suites. Before this file existed, `Dialog.tsx` had no test of
 * its own: its behaviour was only ever observed indirectly, which is how two
 * `aria-modal` dialogs (the git branch dialog over its own confirmation) went
 * unnoticed.
 */

function open(role: OverlayRole = 'standard'): void {
  render(
    <Overlay
      open
      role={role}
      title="A dialog"
      description="One supporting line."
      onClose={() => undefined}
      footerNote="What this commits."
      footer={<button type="button">Confirm</button>}
    >
      <p>Body copy.</p>
    </Overlay>,
  );
}

describe('Overlay', () => {
  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    resetAppState();
  });

  it('renders nothing while closed', () => {
    render(
      <Overlay open={false} title="A dialog" onClose={() => undefined}>
        <p>Body copy.</p>
      </Overlay>,
    );
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.querySelector('.dialog-layer')).toBeNull();
  });

  it('portals into <body> so a fixed layer is never clipped by a transcript row', () => {
    // `.msg` carries `content-visibility: auto`, which makes a row a containing
    // block for fixed descendants: a lightbox opened from a tool card was
    // measured at 691x575 inside a 1440x817 viewport. The layer must be a direct
    // child of <body>, not of whatever rendered the dialog.
    const { container } = render(
      <div class="msg">
        <Overlay open title="A dialog" onClose={() => undefined}>
          <p>Body copy.</p>
        </Overlay>
      </div>,
    );
    expect(container.querySelector('[role="dialog"]')).toBeNull();
    expect(document.body.querySelector(':scope > .dialog-layer')).not.toBeNull();
  });

  it('is one labelled, modal dialog with a described name', () => {
    open();
    const dialog = screen.getByRole('dialog', { name: 'A dialog' });
    expect(dialog.getAttribute('aria-modal')).toBe('true');
    // Exactly one modal surface: the veil, the trap and the scroll lock are all
    // singular, so a second `aria-modal` would be a lie told twice.
    expect(document.querySelectorAll('[aria-modal="true"]')).toHaveLength(1);
    const description = document.getElementById(
      dialog.getAttribute('aria-describedby') ?? '',
    );
    expect(description?.textContent).toBe('One supporting line.');
  });

  it('gives every role its own measure, and never a width from the caller', () => {
    for (const role of [
      'compact',
      'standard',
      'wide',
      'tall',
      'drawer',
      'full',
      'palette',
    ] as const) {
      const { unmount } = render(
        <Overlay open role={role} title="A dialog" onClose={() => undefined}>
          <p>Body copy.</p>
        </Overlay>,
      );
      const dialog = screen.getByRole('dialog');
      expect(dialog.classList.contains('overlay')).toBe(true);
      expect(dialog.classList.contains(`overlay-${role}`)).toBe(true);
      expect(dialog.getAttribute('data-role')).toBe(role);
      unmount();
    }
  });

  it('draws one close button that closes, and a backdrop that closes', () => {
    const onClose = vi.fn();
    render(
      <Overlay open title="A dialog" onClose={onClose}>
        <p>Body copy.</p>
      </Overlay>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
    expect(onClose).toHaveBeenCalledTimes(1);

    fireEvent.click(document.querySelector('.dialog-backdrop') as Element);
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it('keeps the body and the footer as separate bands, in that order', () => {
    open();
    const dialog = screen.getByRole('dialog');
    const bands = [...dialog.children].map((child) => child.className.split(' ')[0]);
    // header, body, footer — the three-band document every modal is.
    expect(bands).toEqual(['overlay-header', 'overlay-body', 'overlay-footer']);
    const footer = dialog.querySelector('.overlay-footer') as HTMLElement;
    expect(footer.querySelector('.overlay-footer-note')?.textContent).toBe(
      'What this commits.',
    );
    expect(footer.querySelector('.overlay-footer-actions')).not.toBeNull();
  });

  it('can drop the body wrapper when the surface owns its own panes', () => {
    // Settings, the Theme studio and the palette each bring a rail or a pane
    // that scrolls itself; a second scroller around them traps the wheel in the
    // wrong one.
    render(
      <Overlay open title="A dialog" onClose={() => undefined} bodyLayout="static">
        <div class="pane">Body copy.</div>
      </Overlay>,
    );
    const body = screen.getByRole('dialog').querySelector('.overlay-body') as HTMLElement;
    expect(body.classList.contains('is-static')).toBe(true);
  });

  it('draws no body wrapper at all in contents mode', () => {
    render(
      <Overlay open title="A dialog" onClose={() => undefined} bodyLayout="contents">
        <div class="pane">Body copy.</div>
      </Overlay>,
    );
    const dialog = screen.getByRole('dialog');
    expect(dialog.querySelector('.overlay-body')).toBeNull();
    expect(dialog.querySelector('.pane')).not.toBeNull();
  });

  it('draws no footer band when a surface has no decision to make', () => {
    render(
      <Overlay open title="A dialog" onClose={() => undefined}>
        <p>Body copy.</p>
      </Overlay>,
    );
    expect(screen.getByRole('dialog').querySelector('.overlay-footer')).toBeNull();
  });

  it('names a headless surface for assistive tech without drawing a header', () => {
    render(
      <Overlay open showHeader={false} title="A dialog" onClose={() => undefined}>
        <p>Body copy.</p>
      </Overlay>,
    );
    expect(screen.getByRole('dialog', { name: 'A dialog' })).not.toBeNull();
    expect(document.querySelector('.overlay-header')).toBeNull();
  });

  it('closes on Escape and hands focus back to whatever opened it', async () => {
    // The trap is installed in the same commit as the surface, so a trusted
    // Escape never reaches the app's shortcut handler. Focus restore is what
    // makes a keyboard-driven dialog usable: the trigger must be where the reader
    // left off.
    function Harness(): preact.JSX.Element {
      const [openState, setOpenState] = useState(false);
      return (
        <div>
          <button type="button" onClick={() => setOpenState(true)}>
            Open it
          </button>
          <Overlay open={openState} title="A dialog" onClose={() => setOpenState(false)}>
            <input aria-label="A field" />
          </Overlay>
        </div>
      );
    }
    render(<Harness />);
    const trigger = screen.getByRole('button', { name: 'Open it' });
    trigger.focus();
    fireEvent.click(trigger);
    const dialog = await screen.findByRole('dialog');
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));

    fireEvent.keyDown(document, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(document.activeElement).toBe(trigger);
  });

  it('Dialogs migrate onto the shell: the old props still name the right measure', () => {
    const { unmount } = render(
      <Dialog open title="Current model" onClose={() => undefined} class="dialog">
        <p>Body copy.</p>
      </Dialog>,
    );
    // The compatibility wrapper keeps the caller's own class *and* adds the
    // role for the measure, so a surface that has not migrated still lands on
    // the one shell.
    expect(screen.getByRole('dialog').classList.contains('dialog')).toBe(true);
    expect(screen.getByRole('dialog').getAttribute('data-role')).toBe('standard');
    unmount();
  });
});
