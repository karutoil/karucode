import type { JSX } from 'preact';
import { useEffect, useMemo } from 'preact/hooks';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { findPanelShapeRenderer, findPluginPanelRenderer, panelPayloadFits, panelPollInterval } from './pluginPanels';
import { RefreshIcon } from './icons';

/** Structural caps apply even when every individual string is tiny. */
export const PANEL_JSON_MAX_DEPTH = 6;
export const PANEL_JSON_MAX_ENTRIES = 40;
export const PANEL_JSON_MAX_NODES = 400;

interface JsonBudget {
  remaining: number;
  truncated: boolean;
}

function jsonTruncation(message: string): JSX.Element {
  return <span class="plugin-panel-json-truncation">{message}</span>;
}

/**
 * Build the bounded VNode tree iteratively at each level. A shared budget is
 * consumed before rendering a value, so hostile breadth cannot enqueue work that
 * will only discover the cap after thousands of child components have mounted.
 */
function jsonValue(value: unknown, depth: number, budget: JsonBudget): JSX.Element {
  budget.remaining -= 1;
  if (value === null || value === undefined) {
    return <span class="plugin-panel-json-null">null</span>;
  }
  if (typeof value === 'string') {
    return <span class="plugin-panel-json-string">{value}</span>;
  }
  if (typeof value === 'number' || typeof value === 'bigint') {
    return <span class="plugin-panel-json-number">{String(value)}</span>;
  }
  if (typeof value === 'boolean') {
    return <span class="plugin-panel-json-boolean">{String(value)}</span>;
  }

  const array = Array.isArray(value);
  const object = !array && typeof value === 'object';
  const entries = array
    ? (value as unknown[]).map((item, index) => [String(index), item] as const)
    : object
      ? Object.entries(value as Record<string, unknown>)
      : [];
  if (array && (value as unknown[]).length === 0) {
    return <span class="plugin-panel-json-empty">[ ]</span>;
  }
  if (object && entries.length === 0) {
    return <span class="plugin-panel-json-empty">{'{ }'}</span>;
  }
  if (!array && !object) {
    return <span class="plugin-panel-json-empty">{typeof value}</span>;
  }
  if (depth >= PANEL_JSON_MAX_DEPTH) {
    budget.truncated = true;
    return jsonTruncation(
      `Depth limit ${PANEL_JSON_MAX_DEPTH} reached; ${entries.length} entr${entries.length === 1 ? 'y is' : 'ies are'} not shown here.`,
    );
  }

  const rows: JSX.Element[] = [];
  const visible = Math.min(entries.length, PANEL_JSON_MAX_ENTRIES);
  if (entries.length > visible) budget.truncated = true;
  for (let index = 0; index < visible; index += 1) {
    if (budget.remaining <= 0) {
      budget.truncated = true;
      break;
    }
    const [key, child] = entries[index]!;
    const drawn = jsonValue(child, depth + 1, budget);
    rows.push(
      array ? (
        <li key={key}>{drawn}</li>
      ) : (
        <div class="plugin-panel-json-row" key={key}>
          <dt>{key}</dt>
          <dd>{drawn}</dd>
        </div>
      ),
    );
  }
  const hidden = entries.length - rows.length;
  if (hidden > 0) {
    budget.truncated = true;
    rows.push(
      array ? (
        <li>{jsonTruncation(
          `${hidden} more ${hidden === 1 ? 'item' : 'items'} not shown (entry or total-node limit).`,
        )}</li>
      ) : (
        <div class="plugin-panel-json-row">
          <dd>{jsonTruncation(
            `${hidden} more ${hidden === 1 ? 'entry' : 'entries'} not shown (entry or total-node limit).`,
          )}</dd>
        </div>
      ),
    );
  }
  return array
    ? <ul class="plugin-panel-json-list">{rows}</ul>
    : <dl class="plugin-panel-json-object">{rows}</dl>;
}

function JsonView({ value }: { value: unknown }): JSX.Element {
  const budget: JsonBudget = { remaining: PANEL_JSON_MAX_NODES, truncated: false };
  const rendered = jsonValue(value, 0, budget);
  return (
    <>
      {rendered}
      {budget.truncated ? (
        <p class="plugin-panel-json-truncation" data-testid="plugin-panel-json-truncated" role="note">
          Preview truncated at {PANEL_JSON_MAX_DEPTH} levels, {PANEL_JSON_MAX_ENTRIES} entries per
          container, or {PANEL_JSON_MAX_NODES} values. The fetched payload is unchanged.
        </p>
      ) : null}
    </>
  );
}

export interface PluginPanelViewProps {
  panel: store.PluginPanelTab;
}

/**
 * One plugin-contributed inspector tab.
 *
 * It renders whatever the plugin answered, three ways in order of preference:
 * a renderer registered for this panel (a first-party component), the generic
 * JSON view, or a readable error. It never renders plugin markup, and it never
 * navigates to a URL the payload suggested.
 */
export function PluginPanelView({ panel }: PluginPanelViewProps): JSX.Element {
  const state = store.pluginPanelData.value[panel.hosted_id];
  const projectId = store.currentProjectId.value;
  const sessionId = store.currentSessionId.value;
  const renderer = useMemo(() => findPluginPanelRenderer(panel), [panel.hosted_id, panel.id]);
  // No renderer for this id: a payload that follows the `view` vocabulary still
  // gets the app's own components (see `PluginViews.tsx`), which is what lets a
  // plugin draw a stat, a bar chart or a live clock without shipping any UI.
  const shapeRenderer = useMemo(
    () => (renderer ? undefined : findPanelShapeRenderer(store.pluginPanelData.value[panel.hosted_id]?.data)),
    [renderer, panel.hosted_id, store.pluginPanelData.value[panel.hosted_id]?.data],
  );
  const drawn = renderer ?? shapeRenderer;
  const interval = panelPollInterval(panel);

  // Read on open, and again whenever the answer could have changed. Both keys
  // are in the dependency list so switching project or session refetches
  // instead of showing the previous scope's data.
  useEffect(() => {
    void actions.refreshPluginPanel(panel);
  }, [panel.hosted_id, projectId, sessionId]);

  useEffect(() => {
    if (interval <= 0) return undefined;
    const timer = window.setInterval(() => {
      void actions.refreshPluginPanel(panel);
    }, interval);
    return () => window.clearInterval(timer);
  }, [panel.hosted_id, interval, projectId, sessionId]);

  const refresh = (): void => {
    void actions.refreshPluginPanel(panel);
  };

  const title = state?.title ?? panel.title;
  const error = state?.error ?? null;
  const data = state?.data;

  let body: JSX.Element;
  if (error) {
    body = (
      <div class="empty-inspector" role="alert">
        <p class="empty-inspector-title">{title} is unavailable</p>
        <p>{error}</p>
        <button type="button" class="button button-small" onClick={refresh}>
          Try again
        </button>
      </div>
    );
  } else if (!state || (state.loading && data === null)) {
    body = (
      <div class="empty-inspector" aria-busy="true">
        <p class="empty-inspector-title">Loading {title.toLowerCase()}…</p>
      </div>
    );
  } else if (!panelPayloadFits(data)) {
    body = (
      <div class="empty-inspector" role="alert">
        <p class="empty-inspector-title">{title} returned too much data to display</p>
        <p>
          A panel payload is capped so one plugin cannot freeze the inspector. Ask the plugin for a
          smaller payload.
        </p>
        <button type="button" class="button button-small" onClick={refresh}>
          Try again
        </button>
      </div>
    );
  } else if (drawn && drawn.matches(data)) {
    body = drawn.render({ panel, plugin: panel.plugin, version: panel.version, data, refresh });
  } else {
    body = (
      <div class="inspector-section">
        <div class="plugin-panel-json-note">
          No renderer is registered for this panel, so the raw payload is shown. It is data, not
          markup: nothing here is executed.
        </div>
        <div class="plugin-panel-json">
          <JsonView value={data} />
        </div>
        {drawn ? (
          <p class="inspector-note">
            A renderer is registered for this panel id but did not claim this payload.
          </p>
        ) : null}
      </div>
    );
  }

  return (
    <div class="inspector-section plugin-panel">
      <div class="inspector-kicker-row">
        <span class="inspector-kicker">
          {title}
          <span class="plugin-panel-badge">
            {panel.plugin} v{panel.version}
          </span>
        </span>
        <button
          type="button"
          class="icon-button plugin-panel-refresh"
          onClick={refresh}
          aria-label={`Refresh ${title}`}
          title="Refresh this panel"
          disabled={state?.loading === true}
        >
          <RefreshIcon size={13} />
        </button>
      </div>
      {body}
      {state?.fetchedAt ? (
        <p class="inspector-note plugin-panel-fetched">
          Read from the plugin at {new Date(state.fetchedAt).toLocaleTimeString()}.
        </p>
      ) : null}
    </div>
  );
}
