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

/**
 * A scroller with a marker and a content element, both measured.
 *
 * `contentTop` is where the start of the loaded rows sits in viewport
 * coordinates: below the viewport top means the reader has scrolled above every
 * row, above it means they are reading inside the list.
 */
function AnchorProbe(props: {
  scrollTop: number;
  scrollHeight: number;
  contentTop: number;
  contentHeight: number;
  canLoad: boolean;
  version: number;
  load: () => Promise<void>;
}): JSX.Element {
  const scroller = useRef<HTMLDivElement>(null);
  const sentinel = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const started = useRef(false);
  useEffect(() => {
    const host = scroller.current;
    if (host) {
      setBox(host, 0, 800);
      setScrollHeight(host, props.scrollHeight);
      // The reader's position is the setup, not a re-render side effect: the
      // hook moves scrollTop itself and the probe must not undo that.
      if (!started.current) {
        started.current = true;
        host.scrollTop = props.scrollTop;
      }
    }
    if (sentinel.current) setBox(sentinel.current, 20, 40);
    if (content.current) setBox(content.current, props.contentTop, props.contentTop + props.contentHeight);
  });
  useHistoryLoader({
    scroller,
    sentinel,
    content,
    canLoad: props.canLoad,
    loading: false,
    version: props.version,
    load: props.load,
  });
  return (
    <div id="anchor-scroller" ref={scroller}>
      <div id="anchor-marker" ref={sentinel} />
      <div id="anchor-content" ref={content} />
    </div>
  );
}

describe('useHistoryLoader with a content anchor', () => {
  it('shows the page that arrived when the reader is above every row', async () => {
    // A huge session: the reader dragged the scrollbar to the top, where only the
    // marker is, and one page of older history is a whole screen and a half.
    let height = 1_000_000;
    const load = vi.fn();

    function Harness(): JSX.Element {
      const [canLoad, setCanLoad] = useState(true);
      const [version, setVersion] = useState(0);
      return (
        <AnchorProbe
          scrollTop={0}
          scrollHeight={height}
          contentTop={60}
          contentHeight={height}
          canLoad={canLoad}
          version={version}
          load={async () => {
            load();
            height += 400_000;
            setVersion((value) => value + 1);
            setCanLoad(false);
          }}
        />
      );
    }

    render(<Harness />);
    await waitFor(() => expect(load).toHaveBeenCalledTimes(1));
    // The page starts at the top of the viewport: the reader reads it from its
    // first row, not 400 000px into history, and the band above the marker is the
    // room they scroll into for the next page.
    await waitFor(() => {
      const scroller = document.getElementById('anchor-scroller') as HTMLDivElement;
      expect(scroller.scrollTop).toBe(60);
    });
    // The page in front of them is not replaced by the next one in the same
    // frame; the next gesture asks for it.
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(load).toHaveBeenCalledTimes(1);
  });

  it('keeps the row being read in place when the reader is inside the rows', async () => {
    let height = 1_000_000;
    const load = vi.fn();

    function Harness(): JSX.Element {
      const [canLoad, setCanLoad] = useState(true);
      const [version, setVersion] = useState(0);
      return (
        <AnchorProbe
          scrollTop={4_000}
          scrollHeight={height}
          contentTop={-20_000}
          contentHeight={height}
          canLoad={canLoad}
          version={version}
          load={async () => {
            load();
            height += 900;
            setVersion((value) => value + 1);
            setCanLoad(false);
          }}
        />
      );
    }

    render(<Harness />);
    await waitFor(() => expect(load).toHaveBeenCalledTimes(1));
    // 900px of older rows were inserted above the reader's line of sight, so the
    // scroll moves by exactly that much and the row stays where it was.
    await waitFor(() => {
      const scroller = document.getElementById('anchor-scroller') as HTMLDivElement;
      expect(scroller.scrollTop).toBe(4_900);
    });
  });
});
