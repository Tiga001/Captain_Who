import type {
  AgentActionExecutionOutput,
  AgentActionIdRequest,
  AgentCancelRunRequest,
  AgentCancelRunResponse,
  AgentCommandSessionGetInput,
  AgentCommandSessionGetOutput,
  AgentCommandSessionListInput,
  AgentCommandSessionListOutput,
  AgentConversationTurnInput,
  AgentConversationTurnOutput,
  AgentConversationTurnRewriteInput,
  AgentContextWindowSnapshotInput,
  AgentContextWindowSnapshotOutput,
  AgentEvent,
  AgentConversationLocator,
  AgentConversationLocatorRequest,
  AgentDetail,
  AgentDetailRequest,
  AgentObserverConversationRequest,
  AgentObserverConversation,
  AgentTemplate,
  AgentTemplateCreateRequest,
  AgentTemplateDeleteRequest,
  AgentTemplateList,
  AgentTemplateListRequest,
  AgentTemplateSetEnabledRequest,
  AgentTemplateUpdateRequest,
  AgentTreeRequest,
  AgentTreeLookup,
  CollaborationApprovalDecisionRequest,
  CollaborationApprovalDecisionResult,
  CollaborationApprovalList,
  CollaborationApprovalListRequest,
  CollaborationEventEnvelope,
  AgentObserverEventEnvelope,
  CollaborationEventsPage,
  CollaborationEventsRequest,
  CollaborationResyncEnvelope,
  AgentFileDraftContentPage,
  AgentFileDraftReadInput,
  AgentFileWriteDiffInput,
  AgentFileWriteDiffPage,
  AgentProviderTransitionNotification,
  AgentProviderTransitionOperation,
  AgentProviderTransitionPreflightInput,
  AgentProviderTransitionPreflightOutput,
  AgentProviderTransitionStartInput,
  AgentProviderTransitionStatusInput,
  AgentProviderTransitionStatusOutput,
  PendingAgentActionSnapshot,
  OfficeEngineStatus,
  AgentRejectActionRequest,
  AgentSteerRunInput,
  AgentSteerRunOutput,
  AgentUsageClearInput,
  AgentUsageClearOutput,
  AgentUsageSummaryInput,
  AgentUsageSummaryOutput,
  AttachmentInputPayload,
  AttachmentSelectInputRequest,
  AutomationAttentionAcknowledgeInput,
  AutomationAttentionAcknowledgeOutput,
  AutomationAttentionSummaryInput,
  AutomationAttentionSummaryOutput,
  AutomationCreateInput,
  AutomationDeleteInput,
  AutomationDeleteOutput,
  AutomationEvent,
  AutomationGetInput,
  AutomationListInput,
  AutomationListOutput,
  AutomationResync,
  AutomationRun,
  AutomationRunNowInput,
  AutomationRunsListInput,
  AutomationRunsListOutput,
  AutomationSetEnabledInput,
  AutomationTask,
  AutomationUpdateInput,
  BrowserArtifactExportInput,
  BrowserArtifactExportOutput,
  BrowserArtifactReadInput,
  BrowserArtifactReadOutput,
  BrowserSurfaceCommand,
  BrowserSurfaceReadyInput,
  BrowserSurfaceReadyOutput,
  BrowserSurfaceSelectedInput,
  BrowserSurfaceSelectedOutput,
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
  GitTurnDiffSummaries,
  GitTurnDiffSummariesInput,
  ManagedArtifactReadIdentity,
  ImageGenerationArtifactReadInput,
  ImageGenerationGetConfigurationOutput,
  ImageGenerationSetEnabledInput,
  ImageGenerationSetEnabledOutput,
  ImageGenerationStatus,
  ImageGenerationUpdateConfigurationInput,
  ImageGenerationUpdateConfigurationOutput,
  McpBuiltinCapabilityListOutput,
  McpBuiltinCapabilityMutationOutput,
  McpBuiltinCapabilitySetAllowedInput,
  McpCatalogToolsPageInput,
  McpCatalogToolsPageOutput,
  McpChangedNotification,
  McpLaunchAuthorizationResult,
  McpServerCreateInput,
  McpServerDetailsOutput,
  McpServerIdInput,
  McpServerListOutput,
  McpServerMutationInput,
  McpServerUpdateInput,
  ProviderProfileUiDescriptor,
  ResourceFaviconRequest,
  ResourceFaviconResponse,
  ChatSearchInput,
  ChatSearchResult,
  SkillInstallationCommitOutput,
  SkillInstallationPreview,
  SkillMutationOutput,
  SkillPreparationCancellationOutput,
  SkillsCancelPreparationInput,
  SkillsCancelSourceResolutionInput,
  SkillsCancelSourceResolutionOutput,
  SkillsChangedNotification,
  SkillsCommitInstallationInput,
  SkillsInspectInstallationInput,
  SkillsListInput,
  SkillsListManagementInput,
  SkillsListManagementOutput,
  SkillsListOutput,
  SkillsResolveInstallationSourceInput,
  SkillsResolveInstallationSourceOutput,
  SkillsSetEnabledInput,
  SkillsSetEnabledOutput,
  SkillsUninstallInput,
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
  StorageChatMessageUiStateRecord,
  StorageComposerDraftRecord,
  StorageImageFileRecord,
  StorageModelSettingsRecord,
  StorageModelSettingsUpdateRecord,
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

export { HOST_CHANNELS } from './channels'

export interface AttachmentsHostApi {
  selectInputAttachments(request: AttachmentSelectInputRequest): Promise<AttachmentInputPayload[]>
}

export interface AutomationsHostApi {
  list(input: AutomationListInput): Promise<HostInvocationResult<AutomationListOutput>>
  get(input: AutomationGetInput): Promise<HostInvocationResult<AutomationTask>>
  create(input: AutomationCreateInput): Promise<HostInvocationResult<AutomationTask>>
  update(input: AutomationUpdateInput): Promise<HostInvocationResult<AutomationTask>>
  setEnabled(input: AutomationSetEnabledInput): Promise<HostInvocationResult<AutomationTask>>
  runNow(input: AutomationRunNowInput): Promise<HostInvocationResult<AutomationRun>>
  delete(input: AutomationDeleteInput): Promise<HostInvocationResult<AutomationDeleteOutput>>
  listRuns(input: AutomationRunsListInput): Promise<HostInvocationResult<AutomationRunsListOutput>>
  attentionSummary(
    input: AutomationAttentionSummaryInput
  ): Promise<HostInvocationResult<AutomationAttentionSummaryOutput>>
  acknowledgeAttention(
    input: AutomationAttentionAcknowledgeInput
  ): Promise<HostInvocationResult<AutomationAttentionAcknowledgeOutput>>
  onEvent(handler: (event: AutomationEvent) => void): () => void
  onResync(handler: (event: AutomationResync) => void): () => void
}

export interface BrowserHostApi {
  clearBrowsingData(): Promise<void>
  /** Opens a native save dialog and exports an exact Host-owned Browser Artifact reference. */
  exportArtifact(
    input: BrowserArtifactExportInput
  ): Promise<HostInvocationResult<BrowserArtifactExportOutput>>
  /** Reads bounded preview bytes for an exact Host-owned Browser Artifact reference. */
  readArtifactPreview(
    input: BrowserArtifactReadInput
  ): Promise<HostInvocationResult<BrowserArtifactReadOutput>>
  /** Acknowledges an exact, already attached right-sidebar browser surface. */
  surfaceReady(input: BrowserSurfaceReadyInput): Promise<BrowserSurfaceReadyOutput>
  /** Reports the visible Browser tab without granting any target or debugger authority. */
  surfaceSelected(input: BrowserSurfaceSelectedInput): Promise<BrowserSurfaceSelectedOutput>
  /** Receives Host-owned reveal/create/close requests without exposing guest or CDP identity. */
  onSurfaceCommand(handler: (command: BrowserSurfaceCommand) => void): () => void
}

export interface OfficeHostApi {
  /** Probes the Office engine shared by Agent tools. */
  getStatus(): Promise<OfficeEngineStatus>
}

export interface StorageHostApi {
  loadModelSettings(): Promise<StorageModelSettingsRecord | null>
  loadProviderProfileUiDescriptors(): Promise<ProviderProfileUiDescriptor[]>
  saveModelSettings(
    settings: StorageModelSettingsUpdateRecord
  ): Promise<HostInvocationResult<StorageModelSettingsRecord>>
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
  loadConversationMetas(): Promise<StorageChatConversationMetaRecord[]>
  loadConversation(conversationId: string): Promise<StorageChatConversationRecord | null>
  loadConversations(): Promise<StorageChatConversationRecord[]>
  forkConversation(
    input: StorageForkConversationRequest
  ): Promise<HostInvocationResult<StorageChatConversationRecord>>
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
  saveChatMessageUiState(input: {
    conversationId: string
    message: StorageChatMessageUiStateRecord
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

export interface ImageGenerationHostApi {
  getConfiguration(): Promise<HostInvocationResult<ImageGenerationGetConfigurationOutput>>
  updateConfiguration(
    input: ImageGenerationUpdateConfigurationInput
  ): Promise<HostInvocationResult<ImageGenerationUpdateConfigurationOutput>>
  setEnabled(
    input: ImageGenerationSetEnabledInput
  ): Promise<HostInvocationResult<ImageGenerationSetEnabledOutput>>
  getStatus(): Promise<HostInvocationResult<ImageGenerationStatus>>
  /** Resolves a private immutable image/PDF Artifact without exposing its managed path. */
  readArtifact(
    input: ImageGenerationArtifactReadInput
  ): Promise<HostInvocationResult<ImageGenerationArtifactContent>>
}

export interface ImageGenerationArtifactContent {
  schemaVersion: 1
  artifact: ManagedArtifactReadIdentity
  fileName: string
  bytes: Uint8Array
}

export interface SkillsHostApi {
  list(input: SkillsListInput): Promise<SkillsListOutput>
  /** Opens a native single-directory picker. Cancellation is not an error. */
  selectInstallationDirectory(): Promise<string | null>
  /** Resolves a user-facing locator into expiring one-time candidate acquisition handles. */
  resolveInstallationSource(
    input: SkillsResolveInstallationSourceInput
  ): Promise<HostInvocationResult<SkillsResolveInstallationSourceOutput>>
  /** Idempotently releases retained candidate authority for a completed or abandoned preview. */
  cancelSourceResolution(
    input: SkillsCancelSourceResolutionInput
  ): Promise<HostInvocationResult<SkillsCancelSourceResolutionOutput>>
  inspectInstallation(
    input: SkillsInspectInstallationInput
  ): Promise<HostInvocationResult<SkillInstallationPreview>>
  commitInstallation(
    input: SkillsCommitInstallationInput
  ): Promise<HostInvocationResult<SkillInstallationCommitOutput>>
  cancelPreparation(
    input: SkillsCancelPreparationInput
  ): Promise<HostInvocationResult<SkillPreparationCancellationOutput>>
  listManagement(
    input: SkillsListManagementInput
  ): Promise<HostInvocationResult<SkillsListManagementOutput>>
  setEnabled(input: SkillsSetEnabledInput): Promise<HostInvocationResult<SkillsSetEnabledOutput>>
  uninstall(input: SkillsUninstallInput): Promise<HostInvocationResult<SkillMutationOutput>>
  onChanged(handler: (event: SkillsChangedNotification) => void): () => void
}

/** Explicit, context-isolated MCP management surface. It intentionally has no direct callTool. */
export interface McpHostApi {
  listBuiltinCapabilities(): Promise<HostInvocationResult<McpBuiltinCapabilityListOutput>>
  setBuiltinCapabilityAllowed(
    input: McpBuiltinCapabilitySetAllowedInput
  ): Promise<HostInvocationResult<McpBuiltinCapabilityMutationOutput>>
  listServers(): Promise<HostInvocationResult<McpServerListOutput>>
  getServer(input: McpServerIdInput): Promise<HostInvocationResult<McpServerDetailsOutput>>
  addServer(input: McpServerCreateInput): Promise<HostInvocationResult<McpServerDetailsOutput>>
  updateServer(input: McpServerUpdateInput): Promise<HostInvocationResult<McpServerDetailsOutput>>
  deleteServer(input: McpServerMutationInput): Promise<HostInvocationResult<McpServerDetailsOutput>>
  requestLaunchAuthorization(
    input: McpServerMutationInput
  ): Promise<HostInvocationResult<McpLaunchAuthorizationResult>>
  enableServer(input: McpServerMutationInput): Promise<HostInvocationResult<McpServerDetailsOutput>>
  disableServer(
    input: McpServerMutationInput
  ): Promise<HostInvocationResult<McpServerDetailsOutput>>
  startServer(input: McpServerMutationInput): Promise<HostInvocationResult<McpServerDetailsOutput>>
  stopServer(input: McpServerMutationInput): Promise<HostInvocationResult<McpServerDetailsOutput>>
  restartServer(
    input: McpServerMutationInput
  ): Promise<HostInvocationResult<McpServerDetailsOutput>>
  getStatus(input: McpServerIdInput): Promise<HostInvocationResult<McpServerDetailsOutput>>
  listTools(
    input: McpCatalogToolsPageInput
  ): Promise<HostInvocationResult<McpCatalogToolsPageOutput>>
  refreshCatalog(
    input: McpServerMutationInput
  ): Promise<HostInvocationResult<McpCatalogToolsPageOutput>>
  /** Native single-file picker. It neither reads nor executes the selected file. */
  selectExecutable(): Promise<string | null>
  /** Native single-directory picker. It neither reads nor starts anything in the directory. */
  selectWorkingDirectory(): Promise<string | null>
  onChanged(handler: (event: McpChangedNotification) => void): () => void
}

export interface GitHostApi {
  inspectRepository(input: GitRepositoryInspectInput): Promise<GitRepositoryInspection>
  getReviewSummary(input: GitReviewSummaryInput): Promise<GitReviewSummary>
  getTurnDiffSummaries(input: GitTurnDiffSummariesInput): Promise<GitTurnDiffSummaries>
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
  getCollaborationTree(input: AgentTreeRequest): Promise<HostInvocationResult<AgentTreeLookup>>
  getCollaborationAgent(input: AgentDetailRequest): Promise<HostInvocationResult<AgentDetail>>
  locateCollaborationConversation(
    input: AgentConversationLocatorRequest
  ): Promise<HostInvocationResult<AgentConversationLocator>>
  loadCollaborationObserverConversation(
    input: AgentObserverConversationRequest
  ): Promise<HostInvocationResult<AgentObserverConversation | null>>
  listCollaborationEvents(
    input: CollaborationEventsRequest
  ): Promise<HostInvocationResult<CollaborationEventsPage>>
  listAgentTemplates(
    input: AgentTemplateListRequest
  ): Promise<HostInvocationResult<AgentTemplateList>>
  createAgentTemplate(
    input: AgentTemplateCreateRequest
  ): Promise<HostInvocationResult<AgentTemplate>>
  updateAgentTemplate(
    input: AgentTemplateUpdateRequest
  ): Promise<HostInvocationResult<AgentTemplate>>
  setAgentTemplateEnabled(
    input: AgentTemplateSetEnabledRequest
  ): Promise<HostInvocationResult<AgentTemplate>>
  deleteAgentTemplate(
    input: AgentTemplateDeleteRequest
  ): Promise<HostInvocationResult<AgentTemplate>>
  listCollaborationApprovals(
    input: CollaborationApprovalListRequest
  ): Promise<HostInvocationResult<CollaborationApprovalList>>
  decideCollaborationApproval(
    input: CollaborationApprovalDecisionRequest
  ): Promise<HostInvocationResult<CollaborationApprovalDecisionResult>>
  onCollaborationEvent(handler: (event: CollaborationEventEnvelope) => void): () => void
  onCollaborationObserverEvent(handler: (event: AgentObserverEventEnvelope) => void): () => void
  onCollaborationResync(handler: (event: CollaborationResyncEnvelope) => void): () => void
  preflightProviderTransition(
    input: AgentProviderTransitionPreflightInput
  ): Promise<HostInvocationResult<AgentProviderTransitionPreflightOutput>>
  startProviderTransition(
    input: AgentProviderTransitionStartInput
  ): Promise<HostInvocationResult<AgentProviderTransitionOperation>>
  getProviderTransitionStatus(
    input: AgentProviderTransitionStatusInput
  ): Promise<HostInvocationResult<AgentProviderTransitionStatusOutput>>
  startConversationTurn(
    input: AgentConversationTurnInput
  ): Promise<HostInvocationResult<AgentConversationTurnOutput>>
  rewriteConversationTurn(
    input: AgentConversationTurnRewriteInput
  ): Promise<HostInvocationResult<AgentConversationTurnOutput>>
  getContextWindowSnapshot(
    input: AgentContextWindowSnapshotInput
  ): Promise<HostInvocationResult<AgentContextWindowSnapshotOutput>>
  listCommandSessions(
    input: AgentCommandSessionListInput
  ): Promise<HostInvocationResult<AgentCommandSessionListOutput>>
  getCommandSession(
    input: AgentCommandSessionGetInput
  ): Promise<HostInvocationResult<AgentCommandSessionGetOutput>>
  steerRun(input: AgentSteerRunInput): Promise<AgentSteerRunOutput>
  cancelRun(input: AgentCancelRunRequest): Promise<AgentCancelRunResponse>
  listPendingActions(): Promise<PendingAgentActionSnapshot[]>
  approveAction(input: AgentActionIdRequest): Promise<AgentActionExecutionOutput>
  rejectAction(input: AgentRejectActionRequest): Promise<AgentActionExecutionOutput>
  cancelAction(input: AgentActionIdRequest): Promise<boolean>
  getUsageSummary(input: AgentUsageSummaryInput): Promise<AgentUsageSummaryOutput>
  clearUsageRecords(input: AgentUsageClearInput): Promise<AgentUsageClearOutput>
  readFileDraft(input: AgentFileDraftReadInput): Promise<AgentFileDraftContentPage>
  getFileWriteDiff(input: AgentFileWriteDiffInput): Promise<AgentFileWriteDiffPage>
  onProviderTransition(handler: (event: AgentProviderTransitionNotification) => void): () => void
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
  automations: AutomationsHostApi
  browser: BrowserHostApi
  git: GitHostApi
  imageGeneration: ImageGenerationHostApi
  mcp: McpHostApi
  office: OfficeHostApi
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
