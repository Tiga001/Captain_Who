// Host API.
import type {
  AgentCancelRunRequest,
  AgentCancelRunResponse,
  AgentStartRunRequest,
  AgentStartRunResponse,
  AppVersionResponse,
  BrowserBounds,
  BrowserCreateViewRequest,
  BrowserNavigateRequest,
  BrowserNavigationState,
  BrowserViewEvent,
  BrowserViewId,
  BrowserZoomState,
  CorePingRequest,
  CorePingResponse,
  StorageAgentPromptPreferencesRecord,
  StorageAppDataSnapshot,
  StorageChatConversationMetaRecord,
  StorageChatConversationRecord,
  StorageChatMessageRecord,
  StorageChatMessageStateRecord,
  StorageComposerDraftRecord,
  StorageModelSettingsRecord,
  StorageProjectRecord,
  StorageUiPreferencesRecord,
  TerminalCreateSessionRequest,
  TerminalExitEvent,
  TerminalOutputEvent,
  TerminalSessionSnapshot
} from '@mycopilot/protocol'

export interface BrowserHostApi {
  createView(request: BrowserCreateViewRequest): Promise<BrowserNavigationState>
  destroyView(id: BrowserViewId): Promise<void>
  setBounds(id: BrowserViewId, bounds: BrowserBounds): Promise<void>
  showView(id: BrowserViewId): Promise<BrowserNavigationState>
  hideView(id: BrowserViewId): Promise<void>
  navigate(request: BrowserNavigateRequest): Promise<BrowserNavigationState>
  reload(id: BrowserViewId): Promise<BrowserNavigationState>
  goBack(id: BrowserViewId): Promise<BrowserNavigationState>
  goForward(id: BrowserViewId): Promise<BrowserNavigationState>
  setZoom(id: BrowserViewId, zoomFactor: number): Promise<BrowserZoomState>
  clearBrowsingData(id: BrowserViewId): Promise<void>
  onEvent(handler: (event: BrowserViewEvent) => void): () => void
}

export interface StorageHostApi {
  loadAppData(): Promise<StorageAppDataSnapshot>
  loadModelSettings(): Promise<StorageModelSettingsRecord | null>
  saveModelSettings(settings: StorageModelSettingsRecord): Promise<void>
  loadAgentPromptPreferences(): Promise<StorageAgentPromptPreferencesRecord>
  saveAgentPromptPreferences(
    preferences: StorageAgentPromptPreferencesRecord
  ): Promise<StorageAgentPromptPreferencesRecord>
  loadProjects(): Promise<StorageProjectRecord[]>
  selectProjectDirectory(): Promise<StorageProjectRecord | null>
  saveProject(project: StorageProjectRecord): Promise<StorageProjectRecord>
  deleteProject(projectId: string): Promise<void>
  showProjectInFolder(projectId: string): Promise<void>
  revealProjectFile(input: { projectId?: string | null; filePath: string }): Promise<void>
  loadConversations(): Promise<StorageChatConversationRecord[]>
  saveConversation(
    conversation: StorageChatConversationRecord
  ): Promise<StorageChatConversationRecord>
  saveConversationMeta(
    conversation: StorageChatConversationMetaRecord
  ): Promise<StorageChatConversationMetaRecord>
  deleteConversation(conversationId: string): Promise<void>
  upsertChatMessages(input: {
    conversationId: string
    messages: StorageChatMessageRecord[]
    positionOffset: number
  }): Promise<StorageChatMessageRecord[]>
  saveChatMessageState(input: {
    conversationId: string
    message: StorageChatMessageStateRecord
  }): Promise<void>
  loadComposerDrafts(): Promise<StorageComposerDraftRecord[]>
  saveComposerDraft(draft: StorageComposerDraftRecord): Promise<StorageComposerDraftRecord>
  deleteComposerDraft(scopeId: string): Promise<void>
  loadUiPreferences(): Promise<StorageUiPreferencesRecord>
  saveUiPreferences(preferences: StorageUiPreferencesRecord): Promise<StorageUiPreferencesRecord>
  selectProfileAvatar(): Promise<string | null>
}

export interface TerminalHostApi {
  createSession(request: TerminalCreateSessionRequest): Promise<TerminalSessionSnapshot>
  writeInput(sessionId: string, data: string): Promise<void>
  resizeSession(sessionId: string, cols: number, rows: number): Promise<void>
  killSession(sessionId: string): Promise<boolean>
  onOutput(handler: (event: TerminalOutputEvent) => void): () => void
  onExit(handler: (event: TerminalExitEvent) => void): () => void
}

export interface HostApi {
  core: {
    ping(input?: CorePingRequest): Promise<CorePingResponse>
  }
  app: {
    getVersion(): Promise<AppVersionResponse>
  }
  agent: {
    startRun(input: AgentStartRunRequest): Promise<AgentStartRunResponse>
    cancelRun(input: AgentCancelRunRequest): Promise<AgentCancelRunResponse>
  }
  browser: BrowserHostApi
  storage: StorageHostApi
  terminal: TerminalHostApi
}

export interface MyCopilotGlobal {
  host: HostApi
}

export function getHostApi(): HostApi {
  const maybeGlobal = globalThis as typeof globalThis & {
    window?: {
      mycopilot?: MyCopilotGlobal
    }
  }
  const host = maybeGlobal.window?.mycopilot?.host

  if (!host) {
    throw new Error('MyCopilot host API is not available')
  }

  return host
}
