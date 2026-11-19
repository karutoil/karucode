import type { JSX } from 'preact';
import type { NotificationsPatch } from '../../api/types';
import {
  NotificationsDeliverySection,
  NotificationsDeviceSection,
  NotificationsSection,
} from '../../components/NotificationsSettings';
import { SettingsAdvanced } from '../../components/SettingsAdvanced';
import * as store from '../../state/store';

export interface NotificationsWrapSectionProps {
  disabled: boolean;
  advancedOpen: boolean;
  onAdvancedChange: (open: boolean) => void;
  onSave: (patch: NotificationsPatch) => Promise<boolean>;
}

export function NotificationsWrapSection({
  disabled,
  advancedOpen,
  onAdvancedChange,
  onSave,
}: NotificationsWrapSectionProps): JSX.Element {
  return (
    <>
      <NotificationsSection
        config={store.config.value}
        disabled={disabled}
        onSave={onSave}
      />

      <NotificationsDeviceSection />

      <SettingsAdvanced
        id="settings-notifications-advanced"
        open={advancedOpen}
        onOpenChange={onAdvancedChange}
        note="Delivery: minimum run length and body text"
      >
        <NotificationsDeliverySection
          config={store.config.value}
          disabled={disabled}
          onSave={onSave}
        />
      </SettingsAdvanced>
    </>
  );
}
