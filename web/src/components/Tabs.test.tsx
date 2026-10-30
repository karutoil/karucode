import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { Tabs, type TabItem } from './Tabs';

/** jsdom has no layout, so each element's box is stubbed by hand. */
function stubBox(element: Element, box: Partial<DOMRect>): void {
  const rect = {
    x: 0,
    y: 0,
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
    width: 0,
    height: 0,
    toJSON: () => ({}),
    ...box,
  } as DOMRect;
  element.getBoundingClientRect = () => rect;
}

function items(count: number): TabItem<string>[] {
  return Array.from({ length: count }, (_, index) => ({
    id: `t${index}`,
    label: `Tab ${index}`,
  }));
}

describe('Tabs', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('renders one tab per item with only the selected one in the tab order', () => {
    render(<Tabs items={items(3)} active="t1" onChange={() => undefined} label="Sections" />);

    const tabs = screen.getAllByRole('tab');
    expect(tabs.map((tab) => tab.textContent)).toEqual(['Tab 0', 'Tab 1', 'Tab 2']);
    expect(tabs.map((tab) => tab.getAttribute('aria-selected'))).toEqual([
      'false',
      'true',
      'false',
    ]);
    expect(tabs.map((tab) => tab.tabIndex)).toEqual([-1, 0, -1]);
  });

  it('moves the selection with the arrow keys and wraps at the ends', () => {
    const onChange = vi.fn();
    render(<Tabs items={items(3)} active="t2" onChange={onChange} label="Sections" />);

    const list = screen.getByRole('tablist');
    fireEvent.keyDown(list, { key: 'ArrowRight' });
    expect(onChange).toHaveBeenLastCalledWith('t0');
    fireEvent.keyDown(list, { key: 'ArrowLeft' });
    expect(onChange).toHaveBeenLastCalledWith('t1');
    fireEvent.keyDown(list, { key: 'Home' });
    expect(onChange).toHaveBeenLastCalledWith('t0');
    fireEvent.keyDown(list, { key: 'End' });
    expect(onChange).toHaveBeenLastCalledWith('t2');
  });

  it('scrolls an off-screen active tab into view', () => {
    // A crowded strip: the list is 360px wide and the selected tab sits past the
    // right edge, which is exactly what a fifth plugin panel produces.
    const { rerender } = render(
      <Tabs items={items(6)} active="t0" onChange={() => undefined} label="Sections" />,
    );

    const list = screen.getByRole('tablist');
    const selected = screen.getAllByRole('tab')[5];
    if (!selected) throw new Error('missing tab');
    stubBox(list, { left: 0, right: 360, width: 360 });
    stubBox(selected, { left: 400, right: 480, width: 80 });

    // Selecting the tab is what a user does (arrow keys, or a panel arriving);
    // it must bring the tab back into the strip's viewport: it hangs 120px past
    // the right edge (480 vs 360), so the strip scrolls by exactly that.
    rerender(<Tabs items={items(6)} active="t5" onChange={() => undefined} label="Sections" />);

    expect(list.scrollLeft).toBe(120);
  });

  it('scrolls a tab that overflows to the left back into view', () => {
    const { rerender } = render(
      <Tabs items={items(6)} active="t5" onChange={() => undefined} label="Sections" />,
    );

    const list = screen.getByRole('tablist');
    const selected = screen.getAllByRole('tab')[0];
    if (!selected) throw new Error('missing tab');
    stubBox(list, { left: 0, right: 360, width: 360 });
    stubBox(selected, { left: -50, right: -17, width: 33 });
    list.scrollLeft = 120;

    rerender(<Tabs items={items(6)} active="t0" onChange={() => undefined} label="Sections" />);

    expect(list.scrollLeft).toBe(70);
  });

  it('leaves the strip alone when the active tab already fits', () => {
    const { rerender } = render(
      <Tabs items={items(6)} active="t0" onChange={() => undefined} label="Sections" />,
    );

    const list = screen.getByRole('tablist');
    const all = screen.getAllByRole('tab');
    for (const [index, tab] of all.entries()) {
      stubBox(tab, { left: index * 40, right: index * 40 + 33, width: 33 });
    }
    stubBox(list, { left: 0, right: 360, width: 360 });
    list.scrollLeft = 120;

    rerender(<Tabs items={items(6)} active="t2" onChange={() => undefined} label="Sections" />);

    expect(list.scrollLeft).toBe(120);
  });
});
