import type { JSX } from 'preact';
import * as actions from '../../state/actions';
import { contextUsage } from '../../state/selectors';
import * as store from '../../state/store';
import { SettingsIcon } from '../../components/icons';
import { Button, LinkButton, StatusDot, Text, type StatusDotTone } from '../../ui';
import { connectionTone } from './helpers';

export function RailFooter(): JSX.Element {
  // A newer build exists. The rail says so next to the version because that is
  // where the version is; opening it lands on Settings → Updates. Once that build
  // is on disk (and this process is still running the old one) the same chip
  // becomes a restart reminder, because an install offered again would re-download
  // what is already there.
  const updateState = store.updateStatus.value;
  const latestBuild = updateState?.update_available ? (updateState.latest?.build ?? null) : null;
  const installedBuild = latestBuild !== null && updateState?.last_applied?.to === String(latestBuild);

  const contextUsageValue = contextUsage(store.chat.value, store.activeContextWindow.value);
  const contextPercent = contextUsageValue
    ? Math.round(contextUsageValue.fraction * 100)
    : null;
  const promptTokens = contextUsageValue?.tokens ?? null;
  const contextWindow = contextUsageValue?.window ?? null;

  const providerLabel = store.providerConfigured.value
    ? store.activeModelLabel.value ?? 'Provider configured'
    : 'No provider';

  return (
    <div class="side-footer">
      {/* The mock's "Workspace context" meter. Ours is the same number the
       * composer's context pill shows - the newest call's prompt over the
       * model's window - so the two can never disagree. It renders nothing
       * when either side is unknown, because a bar drawn from a guess is
       * worse than no bar. */}
      {contextPercent !== null ? (
        <div class="side-meter" title={`${promptTokens?.toLocaleString() ?? '?'} of ${contextWindow?.toLocaleString() ?? '?'} prompt tokens used`}>
          <div class="side-meter-top">
            <span>Context</span>
            <span>{contextPercent}%</span>
          </div>
          <div class="side-meter-bar">
            <i
              style={`width:${Math.max(2, Math.min(100, contextPercent))}%`}
              data-tone={contextPercent >= 90 ? 'hot' : contextPercent >= 70 ? 'warn' : 'ok'}
            />
          </div>
        </div>
      ) : null}
      <div class="online-row">
        <StatusDot tone={connectionTone() as StatusDotTone} class="online-ind" />
        <span>{store.connectionLabel.value}</span>
        <span class="right">
          {store.health.value ? `v${store.health.value.version}` : 'local'}
          {latestBuild !== null ? (
            <LinkButton
              tone="accent"
              class="update-dot ml-3 px-3 border border-line rounded-xs font-mono text-tiny"
              title={
                installedBuild
                  ? `Build ${latestBuild} is installed — restart karucode to run it`
                  : `Build ${latestBuild} is available — open Settings → Updates`
              }
              aria-label={
                installedBuild
                  ? `Restart to run build ${latestBuild}`
                  : `Update available: build ${latestBuild}`
              }
              onClick={() => actions.openSettings('updates')}
            >
              {installedBuild ? '↻' : '↑'}
              {latestBuild}
            </LinkButton>
          ) : null}
        </span>
      </div>
      <div class="settings-row">
        <Button
          variant="ghost"
          size="sm"
          icon={<SettingsIcon size={15} />}
          class="settings-btn"
          onClick={(event) => {
            event.currentTarget.focus();
            actions.openSettings();
          }}
        >
          Settings
        </Button>
        <span class={store.providerConfigured.value ? 'provider' : 'provider is-warn'} title={providerLabel}>
          {providerLabel}
        </span>
      </div>
      {store.connection.value.lastError ? (
        <Text as="p" size="tiny" tone="muted" class="rail-error" title={store.connection.value.lastError}>
          {store.connection.value.lastError}
        </Text>
      ) : null}
    </div>
  );
}
