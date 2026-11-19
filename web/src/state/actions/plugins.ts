import { toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import * as store from '../store';
import { queueChatMessage, sendChatMessage } from './runs';
import {
  enterAuthState,
  isUnauthorized,
  resetPluginPanelData,
  rt,
  type PluginPanelContext,
} from './shared';

export { resetPluginPanelData };

export function selectInspectorTab(tab: store.InspectorTab): void {
  store.inspectorTab.value = tab;
  // A plugin tab reads its payload when it is opened, not before: most sessions
  // never look at one, and a poll would cost a plugin call for nothing.
  const pluginTab = store.pluginPanelTabs.value.find((panel) => panel.hosted_id === tab);
  if (pluginTab) void refreshPluginPanel(pluginTab);
}

function pluginPanelKey(panel: store.PluginPanelTab, context: PluginPanelContext): string {
  // JSON tuple encoding avoids delimiter collisions in arbitrary project/session ids.
  return JSON.stringify([panel.hosted_id, context.projectId, context.sessionId]);
}

function publishPluginPanel(
  panel: store.PluginPanelTab,
  context: PluginPanelContext,
  key: string,
  state: store.PluginPanelState,
): void {
  rt.pluginPanelCache.set(key, state);
  if (
    context.projectId !== store.currentProjectId.value ||
    context.sessionId !== store.currentSessionId.value
  ) {
    return;
  }
  store.pluginPanelData.value = {
    ...store.pluginPanelData.value,
    [panel.hosted_id]: state,
  };
}

/**
 * Read one plugin panel's payload.
 *
 * The route is scoped by project and session like every other project route, so
 * a workspace-scoped plugin is asked about the project the user is looking at
 * rather than about whichever project last ran a prompt.
 *
 * Concurrent calls for the same panel in the same context are coalesced: a poll
 * interval and a manual refresh can overlap without asking the plugin twice.
 * A changed project/session is a different read and gets its own in-flight entry.
 */
export function refreshPluginPanel(panel: store.PluginPanelTab): Promise<void> {
  const context = {
    projectId: store.currentProjectId.value,
    sessionId: store.currentSessionId.value,
  };
  const key = pluginPanelKey(panel, context);
  const epoch = rt.pluginPanelEpoch;
  const existing = rt.pluginPanelInFlight.get(key);
  if (existing && existing.epoch === epoch) return existing.run;

  const cached = rt.pluginPanelCache.get(key);
  let running = true;
  const owner = {};
  const run = (async () => {
    publishPluginPanel(panel, context, key, {
      loading: true,
      // Keep this context's last payload visible while refreshing. Crucially, do
      // not read the hosted-id signal: it may describe another project/session.
      data: cached?.data ?? null,
      title: cached?.title ?? null,
      fetchedAt: cached?.fetchedAt ?? null,
      error: null,
    });
    try {
      const result = await api.getPluginPanel(panel.plugin, panel.id, {
        project: context.projectId,
        session: context.sessionId,
      });
      if (epoch !== rt.pluginPanelEpoch) return;
      publishPluginPanel(panel, context, key, {
        loading: false,
        data: result.data,
        title: result.title || panel.title,
        fetchedAt: result.fetched_ms || Date.now(),
        error: null,
      });
    } catch (error) {
      if (epoch !== rt.pluginPanelEpoch) return;
      publishPluginPanel(panel, context, key, {
        loading: false,
        data: cached?.data ?? null,
        title: cached?.title ?? null,
        fetchedAt: cached?.fetchedAt ?? null,
        error: toErrorMessage(error),
      });
    } finally {
      running = false;
      const active = rt.pluginPanelInFlight.get(key);
      if (active?.owner === owner) rt.pluginPanelInFlight.delete(key);
    }
  })();
  // A synchronously failed request has already cleaned up and must not leave a
  // resolved promise joinable by a future refresh.
  if (running && epoch === rt.pluginPanelEpoch) {
    rt.pluginPanelInFlight.set(key, { run, owner, epoch });
  }
  return run;
}

/**
 * Run one control a plugin contributed: a `ui/action`, or a user turn.
 *
 * This is the click path for every plugin button and palette command, and it is
 * deliberately the *only* path — the component that draws a control does not
 * touch the network, so attribution, in-flight suppression and error reporting
 * live in one place.
 *
 * What happens, in order:
 *
 * 1. a contribution with a `message` sends that turn (queued behind a running
 *    run, exactly like the composer) and does nothing else;
 * 2. a contribution with an `action` posts to
 *    `POST /api/plugins/:name/ui/actions/:action` scoped by the current project
 *    and session;
 * 3. the answer is a toast, a set of panels to re-read, and at most one URL to
 *    open. The URL is re-checked here — `http`/`https` only — before a tab is
 *    opened, because this is where it is actually used.
 *
 * A failed action is shown as a failure and never fakes success. Returns whether
 * anything happened, so a caller can keep a spinner honest.
 */
export async function runPluginContributions(
  contribution: store.PluginUiContribution,
): Promise<boolean> {
  const label = contribution.label;
  if (contribution.message) {
    if (!store.currentSessionId.value) {
      store.pushToast(`Open a session to use “${label}”.`, 'error');
      return false;
    }
    if (store.chat.value.running) {
      const queued = await queueChatMessage('follow_up', { content: contribution.message });
      if (queued) {
        store.pushToast(`Queued “${label}” behind the running turn.`, 'success');
      }
      return queued;
    }
    return sendChatMessage(contribution.message);
  }
  const action = contribution.action;
  if (!action) return false;
  const key = `${contribution.plugin}:${action}`;
  if (rt.pluginUiInFlight.has(key)) return false;
  rt.pluginUiInFlight.add(key);
  try {
    const result = await api.runPluginUiAction(contribution.plugin, action, {
      project: store.currentProjectId.value,
      session: store.currentSessionId.value,
    });
    const message = result.message.trim();
    if (message !== '') {
      store.pushToast(message, result.ok ? 'success' : 'error');
    }
    for (const panel of result.refresh) {
      const tab = store.pluginPanelTabs.value.find(
        (candidate) => candidate.plugin === contribution.plugin && candidate.id === panel,
      );
      if (tab) void refreshPluginPanel(tab);
    }
    if (result.open_url && isOpenableUiUrl(result.open_url)) {
      window.open(result.open_url, '_blank', 'noopener,noreferrer');
    }
    return true;
  } catch (error) {
    if (isUnauthorized(error)) enterAuthState();
    else store.pushToast(`“${label}” failed: ${toErrorMessage(error)}`, 'error');
    return false;
  } finally {
    rt.pluginUiInFlight.delete(key);
  }
}

/**
 * The client-side half of the URL rule: absolute `http`/`https`, nothing else.
 *
 * The host already drops anything else, and this re-checks because *this* is the
 * code that hands the string to the browser.
 */
export function isOpenableUiUrl(url: string): boolean {
  const trimmed = url.trim();
  if (trimmed === '' || trimmed.length > 2_048 || /\s/.test(trimmed)) return false;
  const lower = trimmed.toLowerCase();
  return lower.startsWith('https://') || lower.startsWith('http://');
}
