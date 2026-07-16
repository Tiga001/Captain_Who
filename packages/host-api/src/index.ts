import type {
  AgentActionExecutionOutput,
  AgentActionIdRequest,
  AgentCancelRunRequest,
  AgentCancelRunResponse,
  AgentConversationTurnInput,
  AgentConversationTurnOutput,
  AgentContextCompactionAuditInput,
  AgentContextCompactionAuditOutput,
  AgentContextWindowSnapshotInput,
  AgentContextWindowSnapshotOutput,
  AgentEvent,
  AgentFileDraftContentPage,
  AgentFileDraftReadInput,
  AgentFileWriteDiffInput,
  AgentFileWriteDiffPage,
  PendingAgentActionSnapshot,
  AgentRejectActionRequest,
  AgentUsageClearInput,
  AgentUsageClearOutput,
  AgentUsageSummaryInput,
  AgentUsageSummaryOutput,
  AttachmentInputPayload,
  AttachmentSelectInputRequest,
  CorePingRequest,
  CorePingResponse,
  GitRepositoryInspectInput,
  GitRepositoryInspection,
  GitReviewFileContent,
  GitReviewFileContentInput,
  GitReviewFileDiff,
  GitReviewFileDiffInput,
  GitReviewFileMutation,
  GitReviewFileMutationInput,
  GitReviewSummary,
  GitReviewSummaryInput,
  ResourceFaviconRequest,
  ResourceFaviconResponse,
  ChatSearchInput,
  ChatSearchResult,
  StorageAgentPromptPreferencesRecord,
  StorageAttachmentImageRecord,
  StorageChatConversationMetaRecord,
  StorageChatConversationRecord,
  StorageDeleteChatMessagesRequest,
  StorageForkConversationRequest,
  StorageInputAttachment,
  StorageLoadInputAttachmentsRequest,
  StorageChatMessageRecord,
  StorageChatMessageStateRecord,
  StorageComposerDraftRecord,
  StorageImageFileRecord,
  StorageModelSettingsRecord,
  StorageProjectRecord,
  StorageUiPreferencesRecord,
  TerminalCreateSessionRequest,
  TerminalExitEvent,
  TerminalOutputEvent,
  TerminalSessionSnapshot
} from '@mycopilot/protocol'

export interface AttachmentsHostApi {
  selectInputAttachments(request: AttachmentSelectInputRequest): Promise<AttachmentInputPayload[]>
}

export interface BrowserHostApi {
  clearBrowsingData(): Promise<void>
}

export interface StorageHostApi {
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
  forkConversation(input: StorageForkConversationRequest): Promise<StorageChatConversationRecord>
  saveConversationMeta(
    conversation: StorageChatConversationMetaRecord
  ): Promise<StorageChatConversationMetaRecord>
  deleteConversation(conversationId: string): Promise<void>
  deleteChatMessages(input: StorageDeleteChatMessagesRequest): Promise<void>
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
  loadUiPreferences(): Promise<StorageUiPreferencesRecord>
  saveUiPreferences(preferences: StorageUiPreferencesRecord): Promise<StorageUiPreferencesRecord>
  selectProfileAvatar(): Promise<string | null>
  loadAttachmentImage(input: { attachmentId: string }): Promise<StorageAttachmentImageRecord | null>
  loadInputAttachments(input: StorageLoadInputAttachmentsRequest): Promise<StorageInputAttachment[]>
  loadImageFile(input: {
    projectId?: string | null
    filePath: string
  }): Promise<StorageImageFileRecord | null>
}

export interface SearchHostApi {
  searchChats(input: ChatSearchInput): Promise<ChatSearchResult[]>
}

export interface GitHostApi {
  inspectRepository(input: GitRepositoryInspectInput): Promise<GitRepositoryInspection>
  getReviewSummary(input: GitReviewSummaryInput): Promise<GitReviewSummary>
  getReviewFileDiff(input: GitReviewFileDiffInput): Promise<GitReviewFileDiff>
  getReviewFileContent(input: GitReviewFileContentInput): Promise<GitReviewFileContent>
  mutateReviewFile(input: GitReviewFileMutationInput): Promise<GitReviewFileMutation>
}

export interface ResourcesHostApi {
  resolveFavicon(input: ResourceFaviconRequest): Promise<ResourceFaviconResponse>
}

export interface TerminalHostApi {
  createSession(request: TerminalCreateSessionRequest): Promise<TerminalSessionSnapshot>
  writeInput(sessionId: string, data: string): Promise<void>
  resizeSession(sessionId: string, cols: number, rows: number): Promise<void>
  killSession(sessionId: string): Promise<boolean>
  onOutput(handler: (event: TerminalOutputEvent) => void): () => void
  onExit(handler: (event: TerminalExitEvent) => void): () => void
}

export interface AgentHostApi {
  startConversationTurn(input: AgentConversationTurnInput): Promise<AgentConversationTurnOutput>
  getContextWindowSnapshot(
    input: AgentContextWindowSnapshotInput
  ): Promise<AgentContextWindowSnapshotOutput>
  getContextCompactionAudit(
    input: AgentContextCompactionAuditInput
  ): Promise<AgentContextCompactionAuditOutput>
  cancelRun(input: AgentCancelRunRequest): Promise<AgentCancelRunResponse>
  listPendingActions(): Promise<PendingAgentActionSnapshot[]>
  approveAction(input: AgentActionIdRequest): Promise<AgentActionExecutionOutput>
  rejectAction(input: AgentRejectActionRequest): Promise<AgentActionExecutionOutput>
  cancelAction(input: AgentActionIdRequest): Promise<boolean>
  getUsageSummary(input: AgentUsageSummaryInput): Promise<AgentUsageSummaryOutput>
  clearUsageRecords(input: AgentUsageClearInput): Promise<AgentUsageClearOutput>
  readFileDraft(input: AgentFileDraftReadInput): Promise<AgentFileDraftContentPage>
  getFileWriteDiff(input: AgentFileWriteDiffInput): Promise<AgentFileWriteDiffPage>
  onEvent(handler: (event: AgentEvent) => void): () => void
}

export type NativeThemeSource = 'system' | 'light' | 'dark'

export interface AppWindowState {
  isFullScreen: boolean
  isMaximized: boolean
}

export interface HostApi {
  core: {
    ping(input?: CorePingRequest): Promise<CorePingResponse>
  }
  app: {
    getWindowState(): Promise<AppWindowState>
    openExternal(url: string): Promise<void>
    onWindowStateChange(handler: (state: AppWindowState) => void): () => void
    setNativeThemeSource(themeSource: NativeThemeSource): Promise<void>
  }
  agent: AgentHostApi
  attachments: AttachmentsHostApi
  browser: BrowserHostApi
  git: GitHostApi
  resources: ResourcesHostApi
  search: SearchHostApi
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
