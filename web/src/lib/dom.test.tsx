/* The focus contract shared by every overlay.
 *
 * Two bugs motivated this file and both are invisible without a test that
 * exercises the *DOM*, not the stylesheet:
 *
 *   - `focusableWithin` used `offsetParent !== null` as its visibility gate. In
 *     jsdom that is `null` for every node (there is no layout), so the fallback
 *     had to keep everything and a `display: none` panel's controls were
 *     tabbable. In a browser the opposite failure is possible for subtrees hidden
 *     by `hidden`, `inert` or a closed `<details>`.
 *   - `useFocusTrap` listens on `document` in the capture phase, so on a phone it
 *     saw Escape *before* the sessions sheet's search field, rename input or menu
 *     did — one press on Escape closed the whole sheet instead of clearing the
 *     query. The trap now defers to controls that own the key.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, waitFor } from '@testing-library/preact';
import { render as renderPreact, type ComponentChildren } from 'preact';
import { useRef } from 'preact/hooks';
import { focusableWithin, useFocusTrap } from './dom';

/** Build a detached tree attached to the document, and return its host. */
const hosts: HTMLElement[] = [];

function host(html: string): HTMLElement {
  const element = document.createElement('div');
  element.innerHTML = html;
  document.body.appendChild(element);
  hosts.push(element);
  return element;
}

const ids = (elements: HTMLElement[]): string[] => elements.map((element) => element.id);

afterEach(() => {
  for (const element of hosts.splice(0)) element.remove();
});

describe('focusableWithin: the controls a user can actually reach', () => {
  it('keeps ordinary controls in a layout-less document', () => {
    // The regression: jsdom has no layout at all, so a geometry-only gate
    // classified every control as hidden and the Tab cycle collapsed.
    const tree = host(`
      <button id="button">Go</button>
      <a id="link" href="#x">Link</a>
      <input id="text" />
      <input id="hidden-input" type="hidden" />
      <button id="disabled" disabled>No</button>
      <button id="skip" tabindex="-1">Skip</button>
    `);

    expect(ids(focusableWithin(tree))).toEqual(['button', 'link', 'text']);
  });

  it('skips a display: none subtree, including its descendants', () => {
    const tree = host(`
      <button id="visible">Yes</button>
      <div id="gone" style="display: none">
        <button id="child">No</button>
      </div>
    `);

    expect(ids(focusableWithin(tree))).toEqual(['visible']);
  });

  it('skips a stylesheet-hidden subtree, not just inline styles', () => {
    const style = document.createElement('style');
    style.textContent = '.is-hidden { display: none; }';
    document.head.appendChild(style);
    const tree = host(`
      <button id="visible">Yes</button>
      <div class="is-hidden"><button id="child">No</button></div>
    `);

    expect(ids(focusableWithin(tree))).toEqual(['visible']);
    style.remove();
  });

  it('skips visibility: hidden controls', () => {
    const tree = host(`
      <button id="visible">Yes</button>
      <button id="invisible" style="visibility: hidden">No</button>
    `);

    expect(ids(focusableWithin(tree))).toEqual(['visible']);
  });

  it('skips hidden and inert subtrees by their semantics', () => {
    // Both remove their contents without touching `display`, which is why a
    // computed-style-only check misses them.
    const tree = host(`
      <button id="visible">Yes</button>
      <div hidden><button id="hidden-child">No</button></div>
      <div inert><button id="inert-child">No</button></div>
    `);

    expect(ids(focusableWithin(tree))).toEqual(['visible']);
  });

  it('skips the body of a closed details but keeps its native summary', () => {
    const tree = host(`
      <details>
        <summary id="summary">Summary</summary>
        <button id="body-button">Body</button>
      </details>
    `);

    expect(ids(focusableWithin(tree))).toEqual(['summary']);
  });

  it('keeps a native summary and the details body once it is open', () => {
    const tree = host(`
      <details open>
        <summary id="summary">Summary</summary>
        <button id="body-button">Body</button>
      </details>
    `);

    expect(ids(focusableWithin(tree))).toEqual(['summary', 'body-button']);
  });
});

interface TrapProps {
  active?: boolean;
  onEscape?: () => void;
  label: string;
  children: ComponentChildren;
}

/** A minimal surface using the shared trap, so the hook can be driven directly. */
function Trap({ active = true, onEscape = () => undefined, label, children }: TrapProps) {
  const ref = useRef<HTMLDivElement>(null);
  useFocusTrap(ref, active, onEscape);
  return (
    <div ref={ref} data-testid={label} aria-label={label}>
      {children}
    </div>
  );
}

describe('useFocusTrap: Escape belongs to the control before the surface', () => {
  it('closes the surface on Escape from an ordinary control', () => {
    const onEscape = vi.fn();
    render(
      <Trap label="sheet" onEscape={onEscape}>
        <button>Pin</button>
      </Trap>,
    );

    fireEvent.keyDown(document.querySelector('button')!, { key: 'Escape' });
    expect(onEscape).toHaveBeenCalledTimes(1);
  });

  it('defers to a control marked data-escape-local', () => {
    // The sessions sheet's search field clears, and its rename input cancels,
    // before the sheet considers closing.
    const onEscape = vi.fn();
    const local = vi.fn();
    render(
      <Trap label="sheet" onEscape={onEscape}>
        <input aria-label="Find a session" data-escape-local onKeyDown={local} />
      </Trap>,
    );
    const input = document.querySelector('input')!;
    input.focus();

    fireEvent.keyDown(input, { key: 'Escape' });
    expect(local).toHaveBeenCalledTimes(1);
    expect(onEscape).not.toHaveBeenCalled();
    // The event reached the control instead of being stopped at capture.
    expect(document.activeElement).toBe(input);
  });

  it('defers to a menu only when it explicitly owns Escape', () => {
    const onEscape = vi.fn();
    const local = vi.fn();
    render(
      <Trap label="sheet" onEscape={onEscape}>
        <div role="menu" data-escape-local onKeyDown={local}>
          <button role="menuitem">Rename…</button>
        </div>
      </Trap>,
    );

    fireEvent.keyDown(document.querySelector('[role="menuitem"]')!, { key: 'Escape' });
    expect(local).toHaveBeenCalledTimes(1);
    expect(onEscape).not.toHaveBeenCalled();
  });

  it('closes the surface for a menu role without a local Escape handler', () => {
    const onEscape = vi.fn();
    render(
      <Trap label="sheet" onEscape={onEscape}>
        <div role="menu">
          <button role="menuitem">Rename…</button>
        </div>
      </Trap>,
    );

    fireEvent.keyDown(document.querySelector('[role="menuitem"]')!, { key: 'Escape' });
    fireEvent.keyDown(document.querySelector('[role="menuitem"]')!, { key: 'Escape' });
    expect(onEscape).toHaveBeenCalledTimes(2);
  });

  it('keeps Tab on a native summary when it is the surface’s only focusable', async () => {
    render(
      <Trap label="sheet">
        <details>
          <summary id="only-summary">Details</summary>
          <p>Hidden until opened</p>
        </details>
      </Trap>,
    );
    const summary = document.getElementById('only-summary')!;
    await waitFor(() => expect(document.activeElement).toBe(summary));

    const event = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true });
    summary.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(summary);
  });

  it('wraps Tab over visible controls only', async () => {
    render(
      <Trap label="sheet">
        <button id="first">First</button>
        <button id="hidden" style="display: none">
          Hidden
        </button>
        <button id="last">Last</button>
      </Trap>,
    );
    const first = document.getElementById('first')!;
    const last = document.getElementById('last')!;
    await waitFor(() => expect(document.activeElement).toBe(first));

    fireEvent.keyDown(first, { key: 'Tab', shiftKey: true });
    expect(document.activeElement).toBe(last);
  });

  it('arms Escape in the same commit that exposes a new surface', () => {
    // Raw Preact rendering deliberately does not flush passive effects. Chromium
    // can deliver a trusted Escape in that gap: the dialog is already visible but
    // a useEffect-based trap has not installed its document listener yet.
    const mount = host('');
    const onEscape = vi.fn();
    try {
      renderPreact(
        <Trap label="settings" onEscape={onEscape}>
          <button>Close dialog</button>
        </Trap>,
        mount,
      );

      fireEvent.keyDown(mount.querySelector('button')!, { key: 'Escape' });
      expect(onEscape).toHaveBeenCalledTimes(1);
    } finally {
      renderPreact(null, mount);
    }
  });

  it('hands Escape to a replacement surface without a passive-effect gap', async () => {
    const mount = host('');
    const outer = vi.fn();
    const inner = vi.fn();
    try {
      renderPreact(
        <Trap key="outer" label="outer" onEscape={outer}>
          <button>Outer</button>
        </Trap>,
        mount,
      );
      await waitFor(() => expect(document.activeElement?.textContent).toBe('Outer'));

      renderPreact(
        <Trap key="inner" label="inner" onEscape={inner}>
          <button>Inner</button>
        </Trap>,
        mount,
      );
      fireEvent.keyDown(mount.querySelector('button')!, { key: 'Escape' });

      expect(inner).toHaveBeenCalledTimes(1);
      expect(outer).not.toHaveBeenCalled();
    } finally {
      renderPreact(null, mount);
    }
  });
});

describe('useFocusTrap: the page behind a surface does not pan', () => {
  const originalScrollY = Object.getOwnPropertyDescriptor(window, 'scrollY');

  afterEach(() => {
    if (originalScrollY) Object.defineProperty(window, 'scrollY', originalScrollY);
    document.body.style.overflow = '';
    document.documentElement.style.overflow = '';
    document.body.style.overscrollBehavior = '';
    document.documentElement.style.overscrollBehavior = '';
  });

  it('locks the scroll and puts the document back where the reader left it', async () => {
    const scrollTo = vi.spyOn(window, 'scrollTo').mockImplementation(() => undefined);
    let scrollY = 640;
    // jsdom has no layout: `scrollY` is the only part of the lock it can answer.
    Object.defineProperty(window, 'scrollY', { configurable: true, get: () => scrollY });
    document.body.style.overflow = 'auto';

    const view = render(
      <Trap label="sheet">
        <button>Pin</button>
      </Trap>,
    );
    await waitFor(() =>
      expect(document.activeElement).toBe(document.querySelector('[aria-label="sheet"] button')),
    );

    expect(document.body.style.overflow).toBe('hidden');
    expect(document.documentElement.style.overflow).toBe('hidden');
    expect(document.body.style.overscrollBehavior).toBe('contain');
    expect(document.documentElement.style.overscrollBehavior).toBe('contain');

    // Some mobile browsers reset the document position when the lock is released.
    scrollY = 0;
    view.unmount();

    expect(document.body.style.overflow).toBe('auto');
    expect(document.documentElement.style.overflow).toBe('');
    expect(document.body.style.overscrollBehavior).toBe('');
    expect(document.documentElement.style.overscrollBehavior).toBe('');
    expect(scrollTo).toHaveBeenCalledWith(0, 640);
  });

  it('keeps the lock through a nested surface and releases it once', async () => {
    let scrollY = 240;
    Object.defineProperty(window, 'scrollY', { configurable: true, get: () => scrollY });
    const scrollTo = vi.spyOn(window, 'scrollTo').mockImplementation(() => undefined);

    const view = render(
      <Trap label="outer">
        <button>Outer</button>
      </Trap>,
    );
    view.rerender(
      <>
        <Trap label="outer">
          <button>Outer</button>
        </Trap>
        <Trap label="inner">
          <button>Inner</button>
        </Trap>
      </>,
    );
    expect(document.body.style.overflow).toBe('hidden');

    // The inner one closing must not unlock the page under the outer one.
    view.rerender(
      <Trap label="outer">
        <button>Outer</button>
      </Trap>,
    );
    expect(document.body.style.overflow).toBe('hidden');

    // The reader was at 240; whatever the browser parked the page at, it goes back.
    scrollY = 0;
    view.unmount();
    expect(document.body.style.overflow).toBe('');
    expect(scrollTo).toHaveBeenCalledWith(0, 240);
  });

  it('does not let the landmark focus undo the restored position', async () => {
    // `Element.focus()` pans its target into view by default: focusing the
    // landmark that stands in for a removed opener, right after putting the
    // document back where the reader had it, moved the page a second time.
    // jsdom does not scroll, so the argument passed to `focus` is the observable.
    let scrollY = 800;
    Object.defineProperty(window, 'scrollY', { configurable: true, get: () => scrollY });
    const scrollTo = vi.spyOn(window, 'scrollTo').mockImplementation(() => undefined);

    const main = document.createElement('main');
    main.id = 'shell-main';
    document.body.appendChild(main);
    hosts.push(main);
    const landmarkFocus = vi.spyOn(main, 'focus');

    const opener = document.createElement('button');
    opener.textContent = 'Open';
    document.body.appendChild(opener);
    hosts.push(opener);
    opener.focus();

    const { unmount } = render(
      <Trap label="sheet">
        <button id="inside">Inside</button>
      </Trap>,
    );
    await waitFor(() => expect(document.activeElement).toBe(document.getElementById('inside')));

    // The row this surface was opened from is gone by the time it closes.
    opener.remove();
    scrollY = 0;
    unmount();

    expect(scrollTo).toHaveBeenCalledWith(0, 800);
    expect(landmarkFocus).toHaveBeenCalledTimes(1);
    expect(landmarkFocus).toHaveBeenCalledWith({ preventScroll: true });
    expect(scrollTo.mock.invocationCallOrder[0]!).toBeLessThan(
      landmarkFocus.mock.invocationCallOrder[0]!,
    );
    scrollTo.mockRestore();
  });
});

describe('useFocusTrap: nesting and focus restoration', () => {
  it('lets only the topmost surface handle Escape', () => {
    const outer = vi.fn();
    const inner = vi.fn();
    render(
      <>
        <Trap label="outer" onEscape={outer}>
          <button>Outer</button>
        </Trap>
        <Trap label="inner" onEscape={inner}>
          <button>Inner</button>
        </Trap>
      </>,
    );

    fireEvent.keyDown(document.querySelector('[aria-label="inner"] button')!, { key: 'Escape' });
    expect(inner).toHaveBeenCalledTimes(1);
    expect(outer).not.toHaveBeenCalled();
  });

  it('restores focus to the trigger when it unmounts', async () => {
    const trigger = document.createElement('button');
    trigger.textContent = 'Open';
    document.body.appendChild(trigger);
    trigger.focus();

    const { unmount } = render(
      <Trap label="sheet">
        <button id="inside">Inside</button>
      </Trap>,
    );
    await waitFor(() => expect(document.activeElement).toBe(document.getElementById('inside')));

    unmount();
    await waitFor(() => expect(document.activeElement).toBe(trigger));
  });

  // Reported as: open a dialog from a transcript row, let the run stream and the
  // window swap that row out, close with Escape — and the next Tab starts at the
  // project rail, because focus fell to `body` with nowhere to go.
  it('focuses a landmark when the opener was removed while the surface was open', async () => {
    const main = document.createElement('main');
    main.id = 'shell-main';
    main.innerHTML = '<button id="first">First</button>';
    document.body.appendChild(main);
    hosts.push(main);

    const opener = document.createElement('button');
    opener.textContent = 'Open';
    document.body.appendChild(opener);
    hosts.push(opener);
    opener.focus();

    const { unmount } = render(
      <Trap label="sheet">
        <button id="inside">Inside</button>
      </Trap>,
    );
    await waitFor(() => expect(document.activeElement).toBe(document.getElementById('inside')));

    // A rerender replaced the row the surface was opened from.
    opener.remove();

    unmount();
    expect(document.activeElement).not.toBe(document.body);
    expect(document.activeElement).toBe(main);
    // Focusable by programmatic focus only: the next Tab starts at its first
    // control rather than at the top of the document.
    expect(main.getAttribute('tabindex')).toBe('-1');
  });

  it('returns the stack to the outer surface when the inner one closes', async () => {
    const outer = vi.fn();
    const inner = vi.fn();
    const view = render(
      <Trap label="outer" onEscape={outer}>
        <button>Outer</button>
      </Trap>,
    );
    view.rerender(
      <>
        <Trap label="outer" onEscape={outer}>
          <button>Outer</button>
        </Trap>
        <Trap label="inner" onEscape={inner}>
          <button>Inner</button>
        </Trap>
      </>,
    );

    fireEvent.keyDown(document.querySelector('[aria-label="inner"] button')!, { key: 'Escape' });
    expect(inner).toHaveBeenCalledTimes(1);
    expect(outer).not.toHaveBeenCalled();

    view.rerender(
      <Trap label="outer" onEscape={outer}>
        <button>Outer</button>
      </Trap>,
    );
    fireEvent.keyDown(document.querySelector('[aria-label="outer"] button')!, { key: 'Escape' });
    expect(outer).toHaveBeenCalledTimes(1);
  });
});