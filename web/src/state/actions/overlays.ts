import { isNarrowViewport } from '../../lib/dom';
import * as store from '../store';
import * as workbench from '../workbench';
import * as workspaceTabs from '../workspaceTabs';
import { openSessionInProject } from './projects';
import { replaceOverlay } from './shared';

export function toggleRail(): void {
  if (isNarrowViewport()) {
    store.mobileSheet.value = store.mobileSheet.value === 'sessions' ? null : 'sessions';
    return;
  }
  const next = !store.railCollapsed.value;
  store.railCollapsed.value = next;
  store.persistBool('rail', next);
}

export function toggleInspector(): void {
  if (isNarrowViewport()) {
    store.mobileSheet.value = store.mobileSheet.value === 'inspector' ? null : 'inspector';
    return;
  }
  const next = !store.inspectorCollapsed.value;
  store.inspectorCollapsed.value = next;
  store.persistBool('inspector', next);
}

/**
 * Change how Results is presented (docked column or pop-out overlay).
 *
 * The two modes are different layouts, so carrying one's open/closed state into
 * the other would land the reader in a state neither asked for: a pop-out starts
 * closed (the point of it is that nothing covers the conversation until asked),
 * and going back to docked shows the panel that the docked layout expects. The
 * choice and the open/closed state are both remembered per browser.
 */
export function setInspectorMode(mode: store.InspectorMode): void {
  if (store.inspectorMode.value === mode) return;
  store.inspectorMode.value = mode;
  store.persistInspectorMode(mode);
  const collapsed = mode === 'popout';
  store.inspectorCollapsed.value = collapsed;
  store.persistBool('inspector', collapsed);
}

/**
 * Choose whether the desktop project dropdown opens on hover or on a press.
 *
 * Nothing else changes with it: the list, its rows, its keyboard contract and
 * its close rules are the same in both modes. Only the trigger's pointer
 * behaviour differs, which is why this is one stored word and not a second
 * component. The choice is remembered per browser, like the theme.
 */
export function setProjectMenuOpen(mode: store.ProjectMenuOpen): void {
  if (store.projectMenuOpen.value === mode) return;
  store.projectMenuOpen.value = mode;
  store.persistProjectMenuOpen(mode);
}

/**
 * Choose which key sends a message from the composer.
 *
 * Only the composer's key handling changes: the Send/Queue buttons, the queue
 * toggle, Stop and Escape are the same controls in both modes. The choice is
 * remembered per browser, like the theme, and applies to the next key press -
 * no reload, because the composer reads the signal during its own render.
 */
export function setSendKey(mode: store.SendKey): void {
  if (store.sendKey.value === mode) return;
  store.sendKey.value = mode;
  store.persistSendKey(mode);
}

export function closeSheets(): void {
  workbench.closeContext();
  store.mobileSheet.value = null;
}

export function openSettings(tab?: string): void {
  store.settingsInitialTab.value = tab ?? null;
  replaceOverlay('settings');
}

export function closeSettings(): void {
  store.settingsOpen.value = false;
}

export function togglePalette(): void {
  if (store.paletteOpen.value) {
    store.paletteOpen.value = false;
    return;
  }
  replaceOverlay('palette');
}

export function closePalette(): void {
  store.paletteOpen.value = false;
}

/** Open the Theme Studio drawer, closing any other overlay first. */
export function openThemeStudio(): void {
  replaceOverlay('themeStudio');
}

export function closeThemeStudio(): void {
  store.themeStudioOpen.value = false;
}

export function toggleThemeStudio(): void {
  if (store.themeStudioOpen.value) closeThemeStudio();
  else openThemeStudio();
}

/**
 * Open the notification centre.
 *
 * Mutually exclusive with the other overlays for the same reason they are with each
 * other: two dialogs stacked is one too many, and the notification centre is exactly
 * the thing you open to leave the thing you were doing.
 */
export function openNotificationCenter(): void {
  replaceOverlay('notificationCenter');
}

export function closeNotificationCenter(): void {
  store.notificationCenterOpen.value = false;
}

export function toggleNotificationCenter(): void {
  if (store.notificationCenterOpen.value) closeNotificationCenter();
  else openNotificationCenter();
}

/**
 * Open the session a notification is about, and mark it read.
 *
 * The session may belong to another project, which is the whole point of the
 * centre: a run finished somewhere you are not looking.
 */
export async function openNotification(
  entry: Pick<store.NotificationEntry, 'id' | 'sessionId' | 'projectId'>,
): Promise<void> {
  store.markNotificationRead(entry.id);
  closeNotificationCenter();
  if (entry.sessionId === '') return;
  await openSessionInProject(entry.projectId, entry.sessionId);
}

export function openFileViewer(path: string): void {
  replaceOverlay('fileViewer');
  store.fileViewerPath.value = path;
}

export function closeFileViewer(): void {
  store.fileViewerPath.value = null;
}

/**
 * Open a workspace file as a tab in the centre pane, beside the conversation.
 *
 * The tab is not an overlay: it is a view of the same workspace strip as
 * Conversation, Source control and Terminal, so no other surface is replaced and
 * the modal that may have offered this is closed (the file is now visible where
 * the reader asked to see it).
 */
export function openFileInTab(path: string): void {
  store.fileViewerPath.value = null;
  // A phone sheet is 92vw wide: without this it stays on top of the tab the reader
  // just asked for (`replaceOverlay` clears it for the same reason). Context
  // travels *in* that sheet on a phone, so it is closed through `workbench` first -
  // and only while a sheet is up, so a docked Context column on a wide layout stays
  // exactly where the reader put it.
  if (store.mobileSheet.value !== null) {
    workbench.closeContext();
    store.mobileSheet.value = null;
  }
  workspaceTabs.openFileTab(path);
}

/**
 * Tell the workspace explorer that a file changed on disk from the UI itself, so
 * its cached listing (sizes, and a file the editor just created) catches up.
 */
export function noteWorkspaceFileChanged(_path: string): void {
  store.workspaceRefreshTick.value += 1;
}
