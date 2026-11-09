import { afterEach, describe, expect, it, vi } from 'vitest';
import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { cleanup, render, waitFor } from '@testing-library/preact';
import { useHistoryLoader, sentinelReached } from './historyLoader';

function setBox(element: HTMLElement, top: number, bottom: number): void {
  element.getBoundingClientRect = () =>
    ({
      top,
      bottom,
      height: bottom - top,
      left: 0,
      right: 800,
      width: 800,
      x: 0,
      y: top,
      toJSON: () => ({}),
    }) as DOMRect;
}

function setScrollHeight(element: HTMLElement, value: number): void {
  Object.defineProperty(element, 'scrollHeight', { configurable: true, get: () => value });
}

afterEach(() => cleanup());

describe('sentinelReached', () => {
  const viewport = { top: 100, bottom: 900 };

  it('fires when the marker is in the top band', () => {
    expect(sentinelReached({ top: 100, bottom: 140 }, viewport)).toBe(true);
    expect(sentinelReached({ top: -20, bottom: 120 }, viewport)).toBe(true);
  });

  it('does not fire for a marker far above the viewport', () => {
    // The reader is at the newest content of a long transcript.
    expect(sentinelReached({ top: -5000, bottom: -4960 }, viewport)).toBe(false);
  });

  it('does not fire for a marker below the viewport', () => {
    expect(sentinelReached({ top: 1200, bottom: 1240 }, viewport)).toBe(false);
  });
});

interface ProbeProps {
  canLoad: boolean;
  version: number;
  load: () => Promise<void>;
  scrollHeight: number;
  scrollTop?: number;
  markerTop?: number;
  markerBottom?: number;
}

/** A real DOM scroller and marker with jsdom's missing geometry filled in. */
function Probe(props: ProbeProps): JSX.Element {
  const scroller = useRef<HTMLDivElement>(null);
  const sentinel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const host = scroller.current;
    if (host) {
      setBox(host, 0, 800);
      setScrollHeight(host, props.scrollHeight);
      host.scrollTop = props.scrollTop ?? 0;
    }
    if (sentinel.current) {
      setBox(sentinel.current, props.markerTop ?? 0, props.markerBottom ?? 30);
    }
  }, [props.scrollHeight, props.scrollTop, props.markerTop, props.markerBottom]);
  useHistoryLoader({
    scroller,
    sentinel,
    canLoad: props.canLoad,
    loading: false,
    version: props.version,
    load: props.load,
  });
  return (
    <div id="probe-scroller" ref={scroller}>
      <div ref={sentinel} />
    </div>
  );
}

describe('useHistoryLoader', () => {
  it('loads a page and keeps the reader on the same row when content is prepended', async () => {
    let height = 1_000;
    const load = vi.fn();

    function Harness(): JSX.Element {
      const [canLoad, setCanLoad] = useState(true);
      const [version, setVersion] = useState(0);
      return (
        <Probe
          canLoad={canLoad}
          version={version}
          scrollHeight={height}
          scrollTop={120}
          load={async () => {
            load();
            height += 500;
            setVersion((value) => value + 1);
            setCanLoad(false);
          }}
        />
      );
    }

    render(<Harness />);
    await waitFor(() => expect(load).toHaveBeenCalledTimes(1));
    // The scroller started at 120 and 500px of older rows landed above it, so
    // the same row stays under the reader.
    await waitFor(() => {
      const scroller = document.getElementById('probe-scroller') as HTMLDivElement;
      expect(scroller.scrollTop).toBe(620);
    });
  });

  it('does not spin when a request adds no rows', async () => {
    const load = vi.fn(async () => {});
    render(<Probe canLoad version={0} scrollHeight={1_000} load={load} />);
    await waitFor(() => expect(load).toHaveBeenCalledTimes(1));
    await new Promise((resolve) => setTimeout(resolve, 60));
    // The row count did not change, so the page that "loaded" added nothing and
    // the loop must stop rather than request the same page forever.
    expect(load).toHaveBeenCalledTimes(1);
  });

  it('does not load while the marker is out of view', async () => {
    const load = vi.fn(async () => {});
    render(
      <Probe
        canLoad
        version={0}
        scrollHeight={50_000}
        scrollTop={40_000}
        markerTop={-5_000}
        markerBottom={-4_970}
        load={load}
      />,
    );
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(load).not.toHaveBeenCalled();
  });
});
