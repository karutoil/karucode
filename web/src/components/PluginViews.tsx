import type { JSX } from 'preact';
import type { PluginPanelRenderer } from './pluginPanels';

/**
 * Panel views: a small vocabulary that turns a plugin's JSON into a real view.
 *
 * A plugin could only ever draw the generic JSON tree, because a panel payload is
 * data and the app draws it. That is the right security model and a poor demo, so
 * the app now understands a handful of *shapes*: a payload with a `view` field it
 * knows is drawn by the components below, and anything else keeps the JSON view.
 *
 * This is the whole contract, and it is deliberately tiny:
 *
 * ```json
 * {"view": "stat", "label": "Words", "value": "1,284", "detail": "+12 today", "tone": "ok"}
 * {"view": "list", "title": "Largest files", "items": [{"text": "src/lib.rs", "detail": "1.2 MB", "status": "warn"}]}
 * {"view": "bars", "label": "Files per area", "bars": [{"label": "src", "value": 128}]}
 * {"view": "progress", "label": "Context used", "fraction": 0.62, "detail": "62%"}
 * {"view": "clock", "label": "Focus", "remaining_secs": 1432, "running": true}
 * {"view": "stack", "views": [ …any of the above… ]}
 * ```
 *
 * Every field is plain text or a number; nothing is markup, a URL, a class name or
 * a style. The renderer validates and caps each field, so a wrong shape declines
 * (the JSON view takes over) rather than throwing, and a `stack` is capped in depth
 * and length so a payload cannot make the inspector do unbounded work.
 *
 * `clock` is the one that makes a panel feel alive: give the panel a `refresh_ms`
 * and the countdown ticks, because each poll is a fresh payload.
 */

/** Longest text drawn in one field. Longer is truncated with an ellipsis. */
const MAX_TEXT = 120;
/** Longest a label may be. */
const MAX_LABEL = 40;
/** Rows, bars and nested views are capped so one payload cannot flood a tab. */
const MAX_ITEMS = 60;
const MAX_BARS = 40;
const MAX_STACK = 12;
/** Depth of `stack` nesting. */
const MAX_DEPTH = 3;

type Tone = 'neutral' | 'ok' | 'warn' | 'bad' | 'accent';

function text(value: unknown, cap = MAX_TEXT): string | null {
  if (typeof value === 'number' && Number.isFinite(value)) return String(value);
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  if (trimmed === '') return null;
  return trimmed.length > cap ? `${trimmed.slice(0, Math.max(1, cap - 1))}…` : trimmed;
}

function tone(value: unknown): Tone {
  return value === 'ok' || value === 'warn' || value === 'bad' || value === 'accent'
    ? value
    : 'neutral';
}

/** A number in 0..=1 for the two proportional views. */
function fraction(value: unknown): number | null {
  if (typeof value !== 'number' || !Number.isFinite(value)) return null;
  return Math.min(1, Math.max(0, value));
}

function records(value: unknown, cap: number): Record<string, unknown>[] {
  if (!Array.isArray(value)) return [];
  const out: Record<string, unknown>[] = [];
  for (const entry of value.slice(0, cap)) {
    if (entry !== null && typeof entry === 'object' && !Array.isArray(entry)) {
      out.push(entry as Record<string, unknown>);
    }
  }
  return out;
}

/** Any value a `view` may carry, with the field names a plugin writes. */
function viewKind(data: Record<string, unknown>): string {
  return typeof data.view === 'string' ? data.view.trim().toLowerCase() : '';
}

const KNOWN_VIEWS = ['stat', 'list', 'bars', 'progress', 'clock', 'stack'];

function isKnownView(data: unknown): data is Record<string, unknown> {
  return (
    data !== null &&
    typeof data === 'object' &&
    !Array.isArray(data) &&
    KNOWN_VIEWS.includes(viewKind(data as Record<string, unknown>))
  );
}

function StatView({ data }: { data: Record<string, unknown> }): JSX.Element {
  const label = text(data.label, MAX_LABEL) ?? 'Value';
  const value = text(data.value, 40) ?? '—';
  const detail = text(data.detail);
  return (
    <div class={`plugin-view plugin-view-stat tone-${tone(data.tone)}`}>
      <span class="plugin-view-label">{label}</span>
      <span class="plugin-view-value">{value}</span>
      {detail ? <span class="plugin-view-detail">{detail}</span> : null}
    </div>
  );
}

function ListView({ data }: { data: Record<string, unknown> }): JSX.Element {
  const title = text(data.title, MAX_LABEL);
  const items = records(data.items, MAX_ITEMS);
  return (
    <div class="plugin-view plugin-view-list">
      {title ? <span class="plugin-view-label">{title}</span> : null}
      {items.length === 0 ? (
        <p class="plugin-view-empty">Nothing to show.</p>
      ) : (
        <ul class="plugin-view-rows">
          {items.map((item, index) => {
            const rowText = text(item.text) ?? text(item.label) ?? '';
            const detail = text(item.detail);
            return (
              <li class={`plugin-view-row tone-${tone(item.status)}`} key={`${index}:${rowText}`}>
                <span class="plugin-view-row-text">{rowText}</span>
                {detail ? <span class="plugin-view-row-detail">{detail}</span> : null}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function BarsView({ data }: { data: Record<string, unknown> }): JSX.Element {
  const label = text(data.label, MAX_LABEL);
  const bars = records(data.bars, MAX_BARS)
    .map((bar) => ({
      label: text(bar.label, 16) ?? '',
      value: typeof bar.value === 'number' && Number.isFinite(bar.value) ? Math.max(0, bar.value) : null,
      // What to print beside the bar. The width comes from `value`; a plugin that
      // counts bytes can print "31.2 kB" while the bar still scales honestly.
      display: text(bar.display, 16) ?? null,
      tone: tone(bar.tone),
    }))
    .filter((bar) => bar.value !== null && bar.value > 0);
  const max = bars.reduce((peak, bar) => Math.max(peak, bar.value ?? 0), 0);
  return (
    <div class="plugin-view plugin-view-bars">
      {label ? <span class="plugin-view-label">{label}</span> : null}
      {bars.length === 0 || max <= 0 ? (
        <p class="plugin-view-empty">No data to chart.</p>
      ) : (
        <ul class="plugin-view-bars-list">
          {bars.map((bar, index) => (
            <li class={`plugin-view-bar tone-${bar.tone}`} key={`${index}:${bar.label}`}>
              <span class="plugin-view-bar-label">{bar.label}</span>
              <span
                class="plugin-view-bar-track"
                role="img"
                aria-label={`${bar.label}: ${bar.display ?? bar.value}`}
              >
                {/* The width is the only style the payload can influence, and it is
                 * derived from a number the renderer clamped to the tallest bar, so
                 * a plugin cannot paint outside this row. */}
                <span
                  class="plugin-view-bar-fill"
                  style={{ width: `${Math.round(((bar.value ?? 0) / max) * 100)}%` }}
                />
              </span>
              <span class="plugin-view-bar-value">{bar.display ?? bar.value}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function ProgressView({ data }: { data: Record<string, unknown> }): JSX.Element {
  const label = text(data.label, MAX_LABEL) ?? 'Progress';
  const value = fraction(data.fraction);
  const detail = text(data.detail, 40);
  return (
    <div class={`plugin-view plugin-view-progress tone-${tone(data.tone)}`}>
      <span class="plugin-view-label">
        {label}
        {detail ? <span class="plugin-view-detail">{detail}</span> : null}
      </span>
      <span
        class="plugin-view-progress-track"
        role="progressbar"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={value === null ? undefined : Math.round(value * 100)}
      >
        <span class="plugin-view-progress-fill" style={{ width: `${(value ?? 0) * 100}%` }} />
      </span>
    </div>
  );
}

/** `mm:ss`, or `h:mm:ss` past an hour. Negative and absurd values are clamped. */
export function formatClock(seconds: number | null): string {
  if (seconds === null) return '—:—';
  const total = Math.min(359_999, Math.max(0, Math.round(seconds)));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  const pad = (value: number): string => String(value).padStart(2, '0');
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(secs)}` : `${pad(minutes)}:${pad(secs)}`;
}

function ClockView({ data }: { data: Record<string, unknown> }): JSX.Element {
  const label = text(data.label, MAX_LABEL) ?? 'Timer';
  const remaining =
    typeof data.remaining_secs === 'number' && Number.isFinite(data.remaining_secs)
      ? data.remaining_secs
      : null;
  const running = data.running !== false;
  const detail = text(data.detail);
  return (
    <div class={`plugin-view plugin-view-clock tone-${tone(data.tone)}${running ? ' is-running' : ''}`}>
      <span class="plugin-view-label">{label}</span>
      <span class="plugin-view-clock-face">{formatClock(remaining)}</span>
      {detail ? <span class="plugin-view-detail">{detail}</span> : null}
    </div>
  );
}

/** Draw one `view` document. Exported so `stack` can recurse without re-checking. */
export function renderPluginView(data: unknown, depth = 0): JSX.Element | null {
  if (!isKnownView(data)) return null;
  switch (viewKind(data)) {
    case 'stat':
      return <StatView data={data} />;
    case 'list':
      return <ListView data={data} />;
    case 'bars':
      return <BarsView data={data} />;
    case 'progress':
      return <ProgressView data={data} />;
    case 'clock':
      return <ClockView data={data} />;
    case 'stack': {
      if (depth >= MAX_DEPTH) return null;
      const children = Array.isArray(data.views) ? data.views.slice(0, MAX_STACK) : [];
      return (
        <div class="plugin-view plugin-view-stack">
          {children.map((child, index) => (
            <div class="plugin-view-stack-item" key={index}>
              {renderPluginView(child, depth + 1)}
            </div>
          ))}
        </div>
      );
    }
    default:
      return null;
  }
}

/**
 * The shape renderer the panel view falls back to.
 *
 * It claims any payload with a `view` this build knows; everything else keeps the
 * generic JSON view, so an older app and a newer plugin still read each other.
 */
export const pluginViewRenderer: PluginPanelRenderer = {
  matches: isKnownView,
  render: ({ data }) => (
    <div class="plugin-view-frame">{renderPluginView(data) ?? <p class="plugin-view-empty">Nothing to show.</p>}</div>
  ),
};
