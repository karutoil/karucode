import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render } from '@testing-library/preact';
import { useRef } from 'preact/hooks';
import { useHeightVariable } from './dockHeight';

/* jsdom has no layout, so height and ResizeObserver are both faked: the point of
 * the hook is that the *measured* number reaches CSS, not the measuring. */

class ObserverStub {
  static instances: ObserverStub[] = [];
  readonly targets: Element[] = [];
  private readonly callback: ResizeObserverCallback;

  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
    ObserverStub.instances.push(this);
  }

  observe(target: Element): void {
    this.targets.push(target);
  }

  unobserve(): void {}
  disconnect(): void {
    this.targets.length = 0;
  }

  static reset(): void {
    for (const instance of ObserverStub.instances) instance.disconnect();
    ObserverStub.instances = [];
  }

  /** Reports a new border-box height for every observed element. */
  static fire(height: number): void {
    for (const instance of ObserverStub.instances) {
      instance.callback(
        instance.targets.map((target) => ({
          target,
          borderBoxSize: [{ blockSize: height, inlineSize: 100 }],
          contentRect: { height } as DOMRectReadOnly,
        })) as unknown as ResizeObserverEntry[],
        instance as unknown as ResizeObserver,
      );
    }
  }
}

/** The height jsdom reports for every element; there is no layout engine. */
let layoutHeight = 0;

function Harness({ name = 'composer-h' }: { name?: string }) {
  const measured = useRef<HTMLDivElement>(null);
  const host = useRef<HTMLDivElement>(null);
  useHeightVariable({ ref: measured, target: host, name });
  return (
    <div ref={host}>
      <div ref={measured} />
    </div>
  );
}

describe('useHeightVariable', () => {
  beforeEach(() => {
    ObserverStub.reset();
    vi.stubGlobal('ResizeObserver', ObserverStub);
    // The hook reads `offsetHeight` on mount; jsdom has no layout to produce it.
    layoutHeight = 0;
    Object.defineProperty(HTMLElement.prototype, 'offsetHeight', {
      configurable: true,
      get: () => layoutHeight,
    });
  });

  afterEach(() => {
    ObserverStub.reset();
    vi.unstubAllGlobals();
    Reflect.deleteProperty(HTMLElement.prototype, 'offsetHeight');
  });

  it('publishes the measured height as a custom property', () => {
    layoutHeight = 137;
    const { container } = render(<Harness />);
    expect(container.firstElementChild?.getAttribute('style')).toContain('--composer-h: 137px');
  });

  it('follows the box as it grows and shrinks', () => {
    layoutHeight = 80;
    const { container } = render(<Harness />);
    const host = container.firstElementChild as HTMLElement;
    expect(host.style.getPropertyValue('--composer-h')).toBe('80px');

    ObserverStub.fire(140);
    expect(host.style.getPropertyValue('--composer-h')).toBe('140px');

    // The draft was cleared, or the chip row went away.
    ObserverStub.fire(64);
    expect(host.style.getPropertyValue('--composer-h')).toBe('64px');
  });

  it('rounds to a whole pixel so the observer cannot feed itself', () => {
    layoutHeight = 100;
    const { container } = render(<Harness />);
    ObserverStub.fire(101.6);
    expect(container.firstElementChild?.getAttribute('style')).toContain('--composer-h: 102px');
  });

  it('ignores a zero height instead of collapsing the transcript band', () => {
    layoutHeight = 90;
    const { container } = render(<Harness />);
    const host = container.firstElementChild as HTMLElement;
    expect(host.style.getPropertyValue('--composer-h')).toBe('90px');
    // A hidden tab or an unmounted panel reports 0; writing it would remove the
    // reserved space the dock needs.
    ObserverStub.fire(0);
    expect(host.style.getPropertyValue('--composer-h')).toBe('90px');
  });

  it('removes the property when the dock unmounts', () => {
    layoutHeight = 120;
    const { container, unmount } = render(<Harness />);
    const host = container.firstElementChild as HTMLElement;
    expect(host.style.getPropertyValue('--composer-h')).toBe('120px');
    unmount();
    expect(host.style.getPropertyValue('--composer-h')).toBe('');
  });

  it('uses the name it is given', () => {
    layoutHeight = 50;
    const { container } = render(<Harness name="dock-h" />);
    expect(container.firstElementChild?.getAttribute('style')).toContain('--dock-h: 50px');
  });

  it('still publishes once where ResizeObserver is missing', () => {
    vi.unstubAllGlobals();
    vi.stubGlobal('ResizeObserver', undefined);
    layoutHeight = 77;
    const { container } = render(<Harness />);
    // No observer means no updates, but the dock must not be left without a
    // reserved band on first paint.
    const host = container.firstElementChild as HTMLElement;
    expect(host.style.getPropertyValue('--composer-h')).toBe('77px');
  });
});