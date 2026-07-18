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
  SkillsListInput,
  SkillsListOutput,
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
  TerminalSessionSnapshot,
  WorkspaceDirectoryListing,
  WorkspaceFilePreviewResult,
  WorkspaceFileRequest,
  WorkspaceListDirectoryInput
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

export interface SkillsHostApi {
  list(input: SkillsListInput): Promise<SkillsListOutput>
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
  acknowledgeOutput(sessionId: string, sequence: number): void
  createSession(request: TerminalCreateSessionRequest): Promise<TerminalSessionSnapshot>
  killSession(sessionId: string): Promise<boolean>
  resizeSession(sessionId: string, cols: number, rows: number): Promise<void>
  subscribeSession(sessionId: string, handlers: TerminalSessionEventHandlers): () => void
  writeInput(sessionId: string, data: string): void
}

export interface TerminalSessionEventHandlers {
  onExit(event: TerminalExitEvent): void
  onOutput(event: TerminalOutputEvent): void
}

export interface WorkspaceFilesHostApi {
  copyPath(input: WorkspaceFileRequest): Promise<void>
  listDirectory(input: WorkspaceListDirectoryInput): Promise<WorkspaceDirectoryListing>
  readPreview(input: WorkspaceFileRequest): Promise<WorkspaceFilePreviewResult>
  revealInFolder(input: WorkspaceFileRequest): Promise<void>
}

export interface AgentHostApi {
  startConversationTurn(
    input: AgentConversationTurnInput
  ): Promise<HostInvocationResult<AgentConversationTurnOutput>>
  getContextWindowSnapshot(
    input: AgentContextWindowSnapshotInput
  ): Promise<HostInvocationResult<AgentContextWindowSnapshotOutput>>
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
  skills: SkillsHostApi
  storage: StorageHostApi
  terminal: TerminalHostApi
  workspaceFiles: WorkspaceFilesHostApi
}

export interface MyCopilotGlobal {
  host: HostApi
}

export interface HostInvocationErrorPayload {
  message: string
  code?: number
  data?: unknown
}

/**
 * Serializable Main-to-Renderer result for operations whose structured errors are part of their
 * public contract. Electron otherwise preserves only an IPC handler error's message.
 */
export type HostInvocationResult<T> =
  { ok: true; value: T } | { ok: false; error: HostInvocationErrorPayload }

export class HostInvocationError extends Error {
  readonly code?: number
  readonly data?: unknown

  constructor(payload: HostInvocationErrorPayload) {
    super(payload.message)
    this.name = 'HostInvocationError'
    this.code = payload.code
    this.data = payload.data
  }
}

export async function captureHostInvocation<T>(
  operation: () => Promise<T>
): Promise<HostInvocationResult<T>> {
  try {
    return { ok: true, value: await operation() }
  } catch (error) {
    const record = isRecord(error) ? error : undefined
    const code = typeof record?.code === 'number' ? record.code : undefined
    const data = record && 'data' in record ? record.data : undefined
    return {
      ok: false,
      error: {
        message: error instanceof Error ? error.message : String(error),
        ...(code === undefined ? {} : { code }),
        ...(data === undefined ? {} : { data })
      }
    }
  }
}

export function unwrapHostInvocation<T>(result: HostInvocationResult<T>): T {
  if (result.ok) return result.value
  throw new HostInvocationError(result.error)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object')
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
