import type { JSX } from 'preact';
import type { McpServer, PluginDiagnostics } from '../../api/types';
import { AlertIcon, RefreshIcon } from '../../components/icons';
import { PluginControls } from '../../components/PluginControls';
import { PluginInstallSection } from '../../components/PluginInstallSection';
import { PluginSettingsForm } from '../../components/PluginSettingsForm';
import { PluginToolOutputRow } from '../../components/PluginToolOutput';
import * as store from '../../state/store';
import { Button, Callout, StatusDot, Text } from '../../ui';
import { statusTone } from './providerHelpers';

export interface PluginsMcpSectionProps {
  plugins: PluginDiagnostics[] | null;
  mcpServers: McpServer[] | null;
  diagnosticsError: string | null;
  diagnosticsLoading: boolean;
  pluginBusy: string | null;
  pluginActionError: string | null;
  mcpBusy: string | null;
  mcpActionError: string | null;
  onPluginAction: (name: string, action: 'reload' | 'enable' | 'disable' | 'rescan') => void;
  onMcpAction: (name: string | null) => void;
  onRefreshDiagnostics: () => void;
}

export function PluginsMcpSection({
  plugins,
  mcpServers,
  diagnosticsError,
  diagnosticsLoading,
  pluginBusy,
  pluginActionError,
  mcpBusy,
  mcpActionError,
  onPluginAction,
  onMcpAction,
  onRefreshDiagnostics,
}: PluginsMcpSectionProps): JSX.Element {
  return (
    <>
      <section class="settings-section" aria-labelledby="settings-plugins">
        <div class="settings-section-head">
          <h3 id="settings-plugins">Plugins &amp; MCP</h3>
          <div class="settings-section-actions">
            <Button
              size="sm"
              loading={pluginBusy === 'rescan:all'}
              onClick={() => onPluginAction('all', 'rescan')}
              disabled={pluginBusy !== null}
              title="Re-read every plugin source: start new plugins, drop removed ones, replace changed ones"
            >
              {pluginBusy === 'rescan:all' ? 'Rescanning…' : 'Rescan plugins'}
            </Button>
            <Button
              size="sm"
              loading={mcpBusy === 'all'}
              onClick={() => onMcpAction(null)}
              disabled={mcpBusy !== null}
              title="Re-read [[mcp_servers]]: start new servers, stop removed ones, replace changed ones"
            >
              {mcpBusy === 'all' ? 'Reloading…' : 'Reload MCP'}
            </Button>
            <Button
              size="sm"
              loading={diagnosticsLoading}
              icon={<RefreshIcon size={13} />}
              onClick={onRefreshDiagnostics}
              disabled={diagnosticsLoading}
            >
              Refresh status
            </Button>
          </div>
        </div>
        <Text as="p" class="settings-section-note">
          Plugins and MCP servers run as separate processes with the server's OS permissions.
          {/* Say whether a reload happens on its own: an operator who just
              rebuilt a plugin needs to know whether to click or to wait. */}
          {store.config.value?.plugins_watch
            ? ` Plugin sources are watched every ${store.config.value.plugins_watch_secs ?? 2}s; edits apply without a click.`
            : ' Reload or rescan after changing a plugin. Nothing is applied automatically.'}
          {store.config.value?.mcp_watch
            ? ` MCP entries are watched every ${store.config.value.mcp_watch_secs ?? 2}s; edits apply without a click.`
            : ' MCP entries apply on Reload MCP.'}
        </Text>
        {diagnosticsError ? (
          <Callout tone="danger" role="alert">
            {diagnosticsError}
          </Callout>
        ) : null}
        {pluginActionError ? (
          <Callout tone="danger" role="alert">
            {pluginActionError}
          </Callout>
        ) : null}
        <div class="diagnostics-grid">
          <div class="diagnostics-block">
            <span class="eyebrow">Plugins</span>
            {diagnosticsLoading && plugins === null ? (
              <Text as="p" size="small" tone="muted">
                Loading…
              </Text>
            ) : null}
            {plugins && plugins.length === 0 ? (
              <Text as="p" size="small" tone="muted">
                No plugins loaded.
              </Text>
            ) : null}
            <ul class="diagnostics-list">
              {(plugins ?? []).map((plugin) => (
                <li class="diagnostics-row" key={plugin.name}>
                  <StatusDot tone={statusTone(plugin.status)} />
                  <span class="diagnostics-name">{plugin.name}</span>
                  <span class="diagnostics-status">{plugin.status}</span>
                  <span class="diagnostics-meta">
                    {plugin.manifest?.version ? `v${plugin.manifest.version} · ` : ''}
                    {plugin.tools.length} tool{plugin.tools.length === 1 ? '' : 's'}
                  </span>
                  {plugin.error ? <span class="diagnostics-error">{plugin.error}</span> : null}
                  <PluginToolOutputRow
                    plugin={plugin}
                    onChanged={onRefreshDiagnostics}
                  />
                  <span class="diagnostics-actions">
                    <Button
                      size="sm"
                      loading={pluginBusy === `reload:${plugin.name}`}
                      onClick={() => onPluginAction(plugin.name, 'reload')}
                      disabled={pluginBusy !== null}
                      title="Restart this plugin's process (pick up a rebuilt binary or an edited manifest)"
                    >
                      {pluginBusy === `reload:${plugin.name}` ? 'Reloading…' : 'Reload'}
                    </Button>
                    <Button
                      size="sm"
                      onClick={() =>
                        onPluginAction(
                          plugin.name,
                          plugin.status === 'disabled' ? 'enable' : 'disable',
                        )
                      }
                      disabled={pluginBusy !== null}
                      title="Start or stop this plugin without restarting the server"
                    >
                      {plugin.status === 'disabled' ? 'Enable' : 'Disable'}
                    </Button>
                    {/* The plugin's own schema decides what appears
                      * here; the app supplies no per-plugin form. A
                      * plugin that declares no settings draws nothing. */}
                    <PluginSettingsForm plugin={plugin.name} />
                  </span>
                </li>
              ))}
            </ul>
          </div>
          <div class="diagnostics-block">
            <span class="eyebrow">MCP servers</span>
            {mcpActionError ? (
              <Callout tone="danger" role="alert">
                {mcpActionError}
              </Callout>
            ) : null}
            {diagnosticsLoading && mcpServers === null ? (
              <Text as="p" size="small" tone="muted">
                Loading…
              </Text>
            ) : null}
            {mcpServers && mcpServers.length === 0 ? (
              <Text as="p" size="small" tone="muted">
                No MCP servers configured.
              </Text>
            ) : null}
            <ul class="diagnostics-list">
              {(mcpServers ?? []).map((server) => (
                <li class="diagnostics-row" key={server.name}>
                  <StatusDot tone={statusTone(server.status)} />
                  <span class="diagnostics-name">{server.name}</span>
                  <span class="diagnostics-status">{server.status}</span>
                  <span class="diagnostics-meta">
                    {server.transport} · {server.tools.length} tool
                    {server.tools.length === 1 ? '' : 's'}
                  </span>
                  {server.error ? <span class="diagnostics-error">{server.error}</span> : null}
                  <span class="diagnostics-actions">
                    <Button
                      size="sm"
                      loading={mcpBusy === server.name}
                      onClick={() => onMcpAction(server.name)}
                      disabled={mcpBusy !== null}
                      title="Restart this MCP server's process/connection (pick up a rebuilt binary or an edited entry)"
                    >
                      {mcpBusy === server.name ? 'Reloading…' : 'Reload'}
                    </Button>
                  </span>
                </li>
              ))}
            </ul>
          </div>
        </div>

        {/* Installing from a repository is the other half of
            plugin lifecycle: this block owns its own calls,
            and after a successful job it bumps the same
            diagnostics key the Refresh button uses. */}
        <PluginInstallSection onInstalled={onRefreshDiagnostics} />

        {/* What plugins added to the interface, and what they asked the
          * app to hide. This is the person's side of that bargain: a
          * control nobody wants can be switched off, and a built-in a
          * plugin took away can be put back — without disabling the
          * plugin, which would take its tools and hooks with it. */}
        <PluginControls />
      </section>

      <section class="security-note" aria-labelledby="settings-security-plugins">
        <h3 id="settings-security-plugins">
          <AlertIcon size={15} /> Trusted code
        </h3>
        <p>
          Plugins and MCP servers run as separate processes with the server's OS permissions.
          The status above is informational: their permissions are declarations, not an
          enforced sandbox, so install only what you would run yourself.
        </p>
      </section>
    </>
  );
}
