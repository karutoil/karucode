import type { JSX } from 'preact';
import * as actions from '../state/actions';
import {
  Badge,
  Button,
  Callout,
  IconButton,
  Overlay,
  Skeleton,
  TabPanel,
  Text,
} from '../ui';
import { AccessSettings } from './AccessSettings';
import {
  SETTINGS_SCOPE_LABELS,
  SettingsNav,
  paneCaption,
  searchSettings,
  settingsSection,
  type SettingsTab,
} from './SettingsNav';
import { SkillsSettings } from './SkillsSettings';
import { UpdateSettings } from './UpdateSettings';
import { AlertIcon, CloseIcon } from './icons';
import { AgentSection } from '../features/settings/AgentSection';
import { GeneralSection } from '../features/settings/GeneralSection';
import { ModelsAdvancedSection } from '../features/settings/ModelsAdvancedSection';
import { NotificationsWrapSection } from '../features/settings/NotificationsWrapSection';
import { PluginsMcpSection } from '../features/settings/PluginsMcpSection';
import { ProvidersSection } from '../features/settings/ProvidersSection';
import {
  SCOPE_NOTES,
  SELF_SAVING,
  SELF_SAVING_NOTE,
  tabLabel,
  tabOf,
} from '../features/settings/formModel';
import { useSettingsSheet } from '../features/settings/useSettingsSheet';

export type { SettingsTab };

export function SettingsSheet(): JSX.Element | null {
  const {
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
    pluginsProps,
    reloadConfig,
    selectTab,
    update,
    onAdvancedChange,
    submit,
    saveNotifications,
  } = useSettingsSheet();

  if (!open) return null;

  const results = query.trim() === '' ? [] : searchSettings(query);
  const scope = settingsSection(tab);
  const errorCount = Object.keys(errors).length;
  const errorTabs = [...new Set(Object.keys(errors).map(tabOf))];

  return (
    <Overlay
      open
      title="Settings"
      onClose={actions.closeSettings}
      role="tall"
      class="dialog-settings"
      labelId="settings-title"
      // The sheet draws its own chrome: a rail with the search field and a pane
      // header that names the section on screen and carries the close button.
      showHeader={false}
      bodyLayout="static"
      footer={
        <>
          <Text as="p" class="settings-footer-note">
            {SELF_SAVING.has(tab) ? SELF_SAVING_NOTE : SCOPE_NOTES[scope.scope]}
          </Text>
          {/* Save is only offered where a section is committed by this form. */}
          {SELF_SAVING.has(tab) ? (
            <Button variant="primary" class="min-w-[96px]" onClick={actions.closeSettings}>
              Done
            </Button>
          ) : (
            <>
              <Button variant="secondary" class="min-w-[96px]" onClick={actions.closeSettings}>
                Cancel
              </Button>
              <Button
                variant="primary"
                class="min-w-[96px]"
                disabled={disabled || saving}
                loading={saving}
                onClick={() => void submit()}
              >
                {saving ? 'Saving…' : 'Save changes'}
              </Button>
            </>
          )}
        </>
      }
      footerNote={
        errorCount > 0 ? (
          // The error summary replaces the note in the same slot, so the buttons
          // do not move between the two states.
          <p class="settings-error-summary" role="alert">
            <AlertIcon size={13} />
            <span>
              {errorCount} setting{errorCount === 1 ? '' : 's'} need
              {errorCount === 1 ? 's' : ''} attention in{' '}
              {errorTabs.map(tabLabel).join(', ')}.
            </span>
          </p>
        ) : undefined
      }
    >
      <div class="settings-body">
        <form
          id="settings-form"
          class="settings-form"
          // The sheet validates every field itself and renders inline messages.
          noValidate
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <div class="settings-shell">
            <SettingsNav
              active={tab}
              query={query}
              results={results}
              onQueryChange={setQuery}
              onSelect={selectTab}
            />

            <div class="settings-content">
              <header class="settings-pane-head">
                <div class="settings-pane-headings">
                  <div class="settings-pane-title-row">
                    <Text as="h2" class="settings-pane-title">
                      {scope.label}
                    </Text>
                    <Badge mono={false} class="settings-scope">
                      {SETTINGS_SCOPE_LABELS[scope.scope]}
                    </Badge>
                  </div>
                  {paneCaption(scope) ? (
                    <Text as="p" class="settings-pane-caption">
                      {paneCaption(scope)}
                    </Text>
                  ) : null}
                </div>
                <IconButton
                  label="Close settings"
                  icon={<CloseIcon size={15} />}
                  class="settings-close"
                  onClick={actions.closeSettings}
                />
              </header>

              {loadError ? (
                <Callout
                  tone="danger"
                  role="alert"
                  class="settings-alert"
                  actions={
                    <Button size="sm" onClick={reloadConfig}>
                      Retry
                    </Button>
                  }
                >
                  {loadError}
                </Callout>
              ) : null}

              {locked ? (
                <Callout
                  tone="warn"
                  role="status"
                  icon={<AlertIcon size={15} />}
                  class="settings-alert"
                >
                  <strong>Settings are locked.</strong> The server pins configuration
                  (KARUCODE_LOCK_SETTINGS=1). Changes made here will be rejected.
                </Callout>
              ) : null}

              {saveError ? (
                <Callout
                  tone="danger"
                  role="alert"
                  icon={<AlertIcon size={14} />}
                  title="Could not save settings"
                  class="settings-alert"
                >
                  {saveError}
                </Callout>
              ) : null}

              {/* Panels stay mounted and merely hide so typed values survive tab switches. */}
              <div class="settings-panels">
                {form ? null : (
                  <div class="settings-loading" aria-busy="true" aria-label="Loading settings">
                    {[0, 1, 2, 3, 4, 5].map((index) => (
                      <Skeleton class="skeleton-setting-row" key={index} />
                    ))}
                  </div>
                )}

                {form ? (
                  <>
                    <TabPanel idPrefix="settings" id="general" active={tab === 'general'} persist>
                      <GeneralSection
                        advancedOpen={advanced.general}
                        onAdvancedChange={onAdvancedChange('general')}
                      />
                    </TabPanel>

                    <TabPanel idPrefix="settings" id="models" active={tab === 'models'} persist>
                      <ProvidersSection disabled={disabled} />
                      <ModelsAdvancedSection
                        form={form}
                        errors={errors}
                        disabled={disabled}
                        advancedOpen={advanced.models}
                        onAdvancedChange={onAdvancedChange('models')}
                        update={update}
                      />
                    </TabPanel>

                    <TabPanel idPrefix="settings" id="agent" active={tab === 'agent'} persist>
                      <AgentSection
                        form={form}
                        errors={errors}
                        disabled={disabled}
                        advancedOpen={advanced.agent}
                        onAdvancedChange={onAdvancedChange('agent')}
                        update={update}
                      />
                    </TabPanel>

                    <TabPanel idPrefix="settings" id="skills" active={tab === 'skills'} persist>
                      <SkillsSettings disabled={disabled} />
                    </TabPanel>

                    <TabPanel idPrefix="settings" id="access" active={tab === 'access'} persist>
                      <AccessSettings />
                    </TabPanel>

                    <TabPanel idPrefix="settings" id="notifications" active={tab === 'notifications'} persist>
                      <NotificationsWrapSection
                        disabled={disabled}
                        advancedOpen={advanced.notifications}
                        onAdvancedChange={onAdvancedChange('notifications')}
                        onSave={saveNotifications}
                      />
                    </TabPanel>

                    <TabPanel idPrefix="settings" id="plugins" active={tab === 'plugins'} persist>
                      <PluginsMcpSection {...pluginsProps} />
                    </TabPanel>

                    <TabPanel idPrefix="settings" id="updates" active={tab === 'updates'} persist>
                      <UpdateSettings disabled={disabled} />
                    </TabPanel>
                  </>
                ) : null}
              </div>
            </div>
          </div>
        </form>
      </div>
    </Overlay>
  );
}
