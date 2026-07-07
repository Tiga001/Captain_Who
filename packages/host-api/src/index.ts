// Host API.
import type {
  AgentCancelRunRequest,
  AgentCancelRunResponse,
  AgentStartRunRequest,
  AgentStartRunResponse,
  AppVersionResponse,
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
  create(input?: { id?: string; bounds?: ElectronRectangle }): Promise<{ id: string }>
  navigate(input: { id: string; url: string }): Promise<{ id: string; url: string }>
  setBounds(input: { id: string; bounds: ElectronRectangle }): Promise<{ id: string }>
  destroy(input: { id: string }): Promise<{ id: string }>
}

export interface ElectronRectangle {
  x: number
  y: number
  width: number
  height: number
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
