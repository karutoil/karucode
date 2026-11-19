import { useEffect, useState } from 'preact/hooks';
import { ApiError, toErrorMessage } from '../../api/client';
import {
  listMcpServers,
  listPlugins,
  reloadMcpServer,
  reloadMcpServers,
  reloadPlugin,
  rescanPlugins,
  setPluginEnabled,
} from '../../api/endpoints';
import type { McpServer, NotificationsPatch, PluginDiagnostics } from '../../api/types';
import {
  SETTINGS_SECTIONS,
  matchingSections,
  type SettingsTab,
} from '../../components/SettingsNav';
import * as actions from '../../state/actions';
import * as store from '../../state/store';
import {
  ADVANCED_CLOSED,
  FIELD_LOCATIONS,
  FOUND_FLASH_MS,
  advancedKeyOf,
  buildPatch,
  formFromConfig,
  validate,
} from './formModel';
import type { AdvancedState, FormState } from './types';

export function useSettingsSheet() {
  const open = store.settingsOpen.value;
  const [form, setForm] = useState<FormState | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [locked, setLocked] = useState(false);
  const [tab, setTab] = useState<SettingsTab>('general');
  const [advanced, setAdvanced] = useState<AdvancedState>(ADVANCED_CLOSED);
  const [query, setQuery] = useState('');
  const [jump, setJump] = useState<{ tab: SettingsTab; query: string; nonce: number } | null>(null);
  const [plugins, setPlugins] = useState<PluginDiagnostics[] | null>(null);
  const [mcpServers, setMcpServers] = useState<McpServer[] | null>(null);
  const [diagnosticsError, setDiagnosticsError] = useState<string | null>(null);
  const [diagnosticsLoading, setDiagnosticsLoading] = useState(false);
  const [configReloadKey, setConfigReloadKey] = useState(0);
  const [diagnosticsKey, setDiagnosticsKey] = useState(0);
  const [pluginBusy, setPluginBusy] = useState<string | null>(null);
  const [pluginActionError, setPluginActionError] = useState<string | null>(null);
  const [mcpBusy, setMcpBusy] = useState<string | null>(null);
  const [mcpActionError, setMcpActionError] = useState<string | null>(null);

  /**
   * Run one plugin lifecycle action and re-read the diagnostics afterwards.
   *
   * Reloading is a POST that replaces processes, so the list on screen is stale
   * the moment it returns; bumping `diagnosticsKey` is what makes the row show
   * the new status and pid instead of the old one.
   */
  const runPluginAction = (
    name: string,
    action: 'reload' | 'enable' | 'disable' | 'rescan',
  ): void => {
    setPluginBusy(`${action}:${name}`);
    setPluginActionError(null);
    const request =
      action === 'rescan'
        ? rescanPlugins()
        : action === 'reload'
          ? reloadPlugin(name)
          : setPluginEnabled(name, action === 'enable');
    request
      .then(() => {
        setDiagnosticsKey((key) => key + 1);
        // The diagnostics row is not the only thing that changed: what a plugin
        // *contributes* (its tabs, buttons, palette commands, hides) lives in
        // `/api/config`, which this screen did not otherwise re-read. Without this,
        // installing a plugin would leave its controls invisible until a page
        // reload — the server was already serving them.
        return actions.loadConfig();
      })
      .catch((cause: unknown) => setPluginActionError(toErrorMessage(cause)))
      .finally(() => setPluginBusy(null));
  };

  /**
   * Run one MCP lifecycle action and re-read the diagnostics afterwards.
   *
   * A reload replaces processes, so the list on screen is stale the moment it
   * returns; bumping `diagnosticsKey` is what makes the rows show the new status
   * and tool counts instead of the old ones.
   */
  const runMcpAction = (name: string | null): void => {
    setMcpBusy(name ?? 'all');
    setMcpActionError(null);
    const request = name === null ? reloadMcpServers() : reloadMcpServer(name);
    request
      .then(() => setDiagnosticsKey((key) => key + 1))
      .catch((cause: unknown) => setMcpActionError(toErrorMessage(cause)))
      .finally(() => setMcpBusy(null));
  };

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setErrors({});
    setSaveError(null);
    // A query and a highlight belong to one visit: reopening the sheet is a new
    // question, not a resumed search over a settings page it is no longer on.
    setQuery('');
    setJump(null);
    // A caller that opened the sheet *for* a tab (the rail's update badge says
    // "Settings → Updates") gets that tab. An unknown value falls back to
    // General rather than leaving the sheet on a tab that does not exist.
    const requested = store.settingsInitialTab.value;
    if (requested && SETTINGS_SECTIONS.some((item) => item.id === requested)) {
      setTab(requested as SettingsTab);
    }
    store.settingsInitialTab.value = null;
    // Render from the config already in the store immediately; the fetch below
    // refreshes it (and reports server-side problems) without a blank sheet.
    const cached = store.config.value;
    if (cached) {
      setForm(formFromConfig(cached));
      setLocked(cached.settings_locked);
      setLoading(false);
    } else {
      setLoading(true);
    }
    setLoadError(null);
    actions
      .loadConfig()
      .then((result) => {
        if (cancelled) return;
        setForm(formFromConfig(result));
        setLocked(result.settings_locked);
        setLoading(false);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setLoadError(toErrorMessage(cause));
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, configReloadKey]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setDiagnosticsLoading(true);
    setDiagnosticsError(null);
    Promise.all([listPlugins(), listMcpServers()])
      .then(([pluginList, serverList]) => {
        if (cancelled) return;
        setPlugins(pluginList);
        setMcpServers(serverList);
        setDiagnosticsLoading(false);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setDiagnosticsError(toErrorMessage(cause));
        setDiagnosticsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, diagnosticsKey]);

  /**
   * Scroll to the first section a search reached, once it can be laid out.
   *
   * A section inside a collapsed `<details>` has no box at all, so the fold is
   * opened through state first and the scroll waits one frame for that render.
   * Focusing the section (not the panel) is what announces the arrival: it is a
   * labelled `region`, so a screen reader reads its heading.
   */
  useEffect(() => {
    if (!jump) return;
    const section = matchingSections(jump.tab, jump.query)[0];
    if (!section) return;
    const advancedKey = advancedKeyOf(section.closest('details.settings-advanced'));
    if (advancedKey) {
      setAdvanced((current) =>
        current[advancedKey] ? current : { ...current, [advancedKey]: true },
      );
    }
    const timer = window.setTimeout(() => {
      section.scrollIntoView?.({ block: 'start' });
      section.setAttribute('tabindex', '-1');
      section.focus({ preventScroll: true });
      section.classList.add('is-found');
      window.setTimeout(() => section.classList.remove('is-found'), FOUND_FLASH_MS);
    }, 30);
    return () => window.clearTimeout(timer);
  }, [jump]);

  const disabled = locked || loading || form === null;

  /**
   * Pick a section, and take the user to the query's first hit in it.
   *
   * `nonce` is what makes picking the same result twice work: without it the
   * jump state is unchanged and no effect runs.
   */
  const selectTab = (next: SettingsTab, find: boolean): void => {
    setTab(next);
    if (!find) return;
    setJump((current) => ({ tab: next, query, nonce: (current?.nonce ?? 0) + 1 }));
  };

  const update = (patch: Partial<FormState>): void => {
    setForm((current) => (current ? { ...current, ...patch } : current));
    setErrors((current) => (Object.keys(current).length > 0 ? {} : current));
  };

  const onAdvancedChange = (key: keyof AdvancedState) => (value: boolean): void => {
    setAdvanced((current) => ({ ...current, [key]: value }));
  };

  /**
   * Take the user to the first field the form rejected.
   *
   * Validation covers every tab, so a failure is usually off-screen. Switching
   * tab, expanding the disclosure that holds it and focusing the input is the
   * difference between "fix this" and "find this".
   */
  const revealErrors = (validation: Record<string, string>): void => {
    const first = Object.keys(validation)[0];
    if (!first) return;
    const location = FIELD_LOCATIONS[first];
    if (!location) return;
    setTab(location.tab);
    if (location.advanced) {
      const advancedKey = location.advanced;
      setAdvanced((current) => ({ ...current, [advancedKey]: true }));
    }
    // After the render that switches the tab and opens the disclosure, so the
    // element is visible (a closed <details> hides its own subtree).
    window.setTimeout(() => {
      const element = document.getElementById(location.anchor);
      if (!element) return;
      element.scrollIntoView({ block: 'center' });
      element.focus({ preventScroll: true });
    }, 0);
  };

  const submit = async (): Promise<void> => {
    if (!form || disabled) return;
    const validation = validate(form);
    setErrors(validation);
    if (Object.keys(validation).length > 0) {
      revealErrors(validation);
      return;
    }
    setSaving(true);
    setSaveError(null);
    try {
      const updated = await actions.saveConfig(buildPatch(form));
      setForm(formFromConfig(updated));
      setLocked(updated.settings_locked);
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === 403) {
        setLocked(true);
        setSaveError('Settings are locked by the server (KARUCODE_LOCK_SETTINGS).');
      } else {
        setSaveError(toErrorMessage(cause));
      }
    } finally {
      setSaving(false);
    }
  };

  /**
   * Save one notification switch and re-seed the form from the answer.
   *
   * Notification switches PATCH on their own (they have no Save button of their
   * own), so this reports failure the same way `submit` does: a locked install
   * flips the sheet into its read-only state rather than showing a generic error.
   */
  const saveNotifications = async (patch: NotificationsPatch): Promise<boolean> => {
    try {
      const updated = await actions.saveConfig({ notifications: patch });
      setForm(formFromConfig(updated));
      setLocked(updated.settings_locked);
      return true;
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === 403) {
        setLocked(true);
        setSaveError('Settings are locked by the server (KARUCODE_LOCK_SETTINGS).');
      } else {
        setSaveError(toErrorMessage(cause));
      }
      return false;
    }
  };

  return {
    open,
    form,
    loadError,
    errors,
    saving,
    saveError,
    locked,
    tab,
    advanced,
    query,
    setQuery,
    disabled,
    pluginsProps: {
      plugins,
      mcpServers,
      diagnosticsError,
      diagnosticsLoading,
      pluginBusy,
      pluginActionError,
      mcpBusy,
      mcpActionError,
      onPluginAction: runPluginAction,
      onMcpAction: runMcpAction,
      onRefreshDiagnostics: () => setDiagnosticsKey((key) => key + 1),
    },
    reloadConfig: () => setConfigReloadKey((key) => key + 1),
    selectTab,
    update,
    onAdvancedChange,
    submit,
    saveNotifications,
  };
}
