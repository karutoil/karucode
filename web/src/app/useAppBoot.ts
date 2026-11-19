import { useEffect } from 'preact/hooks';
import * as actions from '../state/actions';
import { controlPlaneOpen } from '../state/controlPlane';
import * as notifyWiring from '../state/notifyWiring';
import * as store from '../state/store';
import * as workbench from '../state/workbench';

export function useAppBoot(): void {
  useEffect(() => {
    const unregisterAuth = actions.registerAuthHandler();
    void actions.bootstrap();

    // Sound needs one interaction before a browser will let it play. Attaching the
    // listener is all this does; it removes itself after the first gesture.
    const disarmSound = notifyWiring.armSoundOnFirstGesture();
    // Keep the engine in step with the persisted preferences, and with a change
    // made in another tab.
    notifyWiring.syncSoundPreferences();
    const onStorage = (event: StorageEvent): void => {
      if (event.key === null || event.key.startsWith('karucode.notify.')) {
        store.soundEnabled.value = store.readSoundEnabled();
        store.soundVolume.value = store.readSoundVolume();
        store.desktopEnabled.value = store.readDesktopEnabled();
        notifyWiring.syncSoundPreferences();
      }
    };
    window.addEventListener('storage', onStorage);

    const clockTimer = window.setInterval(() => {
      store.clock.value = Date.now();
    }, 20_000);
    const healthTimer = window.setInterval(() => {
      void actions.checkHealth();
    }, 20_000);

    const media =
      typeof window.matchMedia === 'function' ? window.matchMedia('(max-width: 1023px)') : null;
    const phoneMedia =
      typeof window.matchMedia === 'function' ? window.matchMedia('(max-width: 640px)') : null;
    const updateViewport = (): void => {
      store.narrowViewport.value = media ? media.matches : false;
      store.phoneViewport.value = phoneMedia ? phoneMedia.matches : false;
      if (media && !media.matches) store.mobileSheet.value = null;
    };
    updateViewport();
    if (media) {
      if (typeof media.addEventListener === 'function') media.addEventListener('change', updateViewport);
      else if (typeof media.addListener === 'function') media.addListener(updateViewport);
    }
    if (phoneMedia) {
      if (typeof phoneMedia.addEventListener === 'function') phoneMedia.addEventListener('change', updateViewport);
      else if (typeof phoneMedia.addListener === 'function') phoneMedia.addListener(updateViewport);
    }

    return () => {
      unregisterAuth();
      disarmSound();
      window.removeEventListener('storage', onStorage);
      window.clearInterval(clockTimer);
      window.clearInterval(healthTimer);
      if (media) {
        if (typeof media.removeEventListener === 'function') media.removeEventListener('change', updateViewport);
        else if (typeof media.removeListener === 'function') media.removeListener(updateViewport);
      }
      if (phoneMedia) {
        if (typeof phoneMedia.removeEventListener === 'function') phoneMedia.removeEventListener('change', updateViewport);
        else if (typeof phoneMedia.removeListener === 'function') phoneMedia.removeListener(updateViewport);
      }
      actions.stopStream();
    };
  }, []);

  useEffect(() => {
    if (controlPlaneOpen.value) {
      workbench.closeContext();
      store.mobileSheet.value = null;
    }
  }, [controlPlaneOpen.value]);

  const session = store.activeSession.value;
  const unreadCount = store.unreadTotal.value;
  useEffect(() => {
    const base = session?.title ? `${session.title} — karucode` : 'karucode';
    // The unread count is the only thing a notification can tell you from another
    // tab, so it goes in the title rather than only in the page.
    document.title = unreadCount > 0 ? `(${unreadCount}) ${base}` : base;
  }, [session?.title, unreadCount]);
}
