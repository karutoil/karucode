import type { JSX } from 'preact';
import { useEffect, useMemo } from 'preact/hooks';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { findPluginPanelRenderer, panelPayloadFits, panelPollInterval } from './pluginPanels';
import { RefreshIcon } from './icons';

/**
 * A generic, read-only view of any JSON document.
 *
 * This is the fallback for a plugin panel the app has no renderer for — which
 * is the normal case, because a plugin ships its payload but never its UI. It is
 * deliberately inert: text only, no HTML, no links built from the payload, no
 * keys turned into URLs. A plugin payload is untrusted input.
 */
function JsonView({ value, depth = 0 }: { value: unknown; depth?: number }): JSX.Element {
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
  if (Array.isArray(value)) {
    if (value.length === 0) {
      return <span class="plugin-panel-json-empty">[ ]</span>;
    }
    return (
      <ul class="plugin-panel-json-list">
        {value.map((item, index) => (
          <li key={index}>
            <JsonView value={item} depth={depth + 1} />
          </li>
        ))}
      </ul>
    );
  }
  if (typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 0) {
      return <span class="plugin-panel-json-empty">{'{ }'}</span>;
    }
    return (
      <dl class="plugin-panel-json-object">
        {entries.map(([key, item]) => (
          <div class="plugin-panel-json-row" key={key}>
            <dt>{key}</dt>
            <dd>
              <JsonView value={item} depth={depth + 1} />
            </dd>
          </div>
        ))}
      </dl>
    );
  }
  // Functions, symbols and anything else: say what it is instead of guessing.
  return <span class="plugin-panel-json-empty">{typeof value}</span>;
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
  } else if (renderer && renderer.matches(data)) {
    body = renderer.render({ panel, plugin: panel.plugin, version: panel.version, data, refresh });
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
        {renderer ? (
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
