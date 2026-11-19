/**
 * Backwards-compatible barrel for `state/actions/*`.
 *
 * Domain implementations live under `./actions/`; this file re-exports every
 * public symbol so existing callers and tests keep importing `state/actions`.
 */

export {
  bootstrap,
  finishOnboarding,
  refreshAccessSettings,
  refreshAuthStatus,
  refreshOnboarding,
  registerAuthHandler,
  resetRuntimeForTests,
  retryConnection,
  signInWithPasskey,
  signInWithPassword,
  signOut,
  submitToken,
} from './actions/bootstrap';
export { fillComposer, materializeDraftSession, startDraftSession } from './actions/composer';
export {
  closeAgentDetail,
  controlAgent,
  loadOlderAgent,
  openAgentDetail,
  refreshAgents,
  refreshSessionAgents,
  setAgentFilter,
  setAgentScope,
} from './actions/control';
export { restartStream, startStream, stopStream } from './actions/events';
export {
  closeFileViewer,
  closeNotificationCenter,
  closePalette,
  closeSettings,
  closeSheets,
  closeThemeStudio,
  noteWorkspaceFileChanged,
  openFileInTab,
  openFileViewer,
  openNotification,
  openNotificationCenter,
  openSettings,
  openThemeStudio,
  setInspectorMode,
  setProjectMenuOpen,
  setSendKey,
  toggleInspector,
  toggleNotificationCenter,
  togglePalette,
  toggleRail,
  toggleThemeStudio,
} from './actions/overlays';
export {
  isOpenableUiUrl,
  refreshPluginPanel,
  resetPluginPanelData,
  runPluginContributions,
  selectInspectorTab,
} from './actions/plugins';
export {
  createProject,
  deleteProjectById,
  focusSession,
  launchSessionWithPrompt,
  openAgentSession,
  openSessionInProject,
  refreshProjects,
  renameProjectById,
  saveProjectResultPanelWidth,
  switchProject,
  type ProjectSwitchOptions,
} from './actions/projects';
export {
  applyRewind,
  closeRewind,
  dismissRunError,
  loadRewindPreview,
  openRewind,
  queueChatMessage,
  recallQueuedMessage,
  resolveApproval,
  runSlashCommand,
  sendChatMessage,
  stopRun,
} from './actions/runs';
export {
  clearSessionModelSelection,
  closeSession,
  deleteSessionById,
  learnStoredMessageIds,
  loadOlderSession,
  openSession,
  refreshSessionInfo,
  refreshSessions,
  renameSessionById,
  scheduleSessionInfoRefresh,
  scheduleSessionsRefresh,
  scheduleStoredMessageIds,
  setSessionModelSelection,
  setSessionWorktrees,
  switchSession,
  type OpenSessionOptions,
} from './actions/sessions';
export {
  applyConfig,
  checkHealth,
  clearApiKey,
  installUpdate,
  loadCatalog,
  loadConfig,
  lookupModel,
  refreshUpdateStatus,
  runUpdateCheck,
  saveConfig,
  saveProviders,
  switchModel,
  testProvider,
} from './actions/settings';
