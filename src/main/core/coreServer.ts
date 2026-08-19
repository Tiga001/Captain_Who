import { createHash } from 'node:crypto'
import type { ImageGenerationArtifactContent } from '@mycopilot/host-api'
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
  AgentObserverEventEnvelope,
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
  AgentRejectActionRequest,
  AgentSteerRunInput,
  AgentSteerRunOutput,
  AgentUsageClearInput,
  AgentUsageClearOutput,
  AgentUsageSummaryInput,
  AgentUsageSummaryOutput,
  PendingAgentActionSnapshot,
  OfficeEngineStatus,
  CorePingRequest,
  CorePingResponse,
  CoreShutdownResponse,
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
  ImageGenerationArtifactReadOutput,
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
  McpLaunchAuthorizationCommitInput,
  McpLaunchAuthorizationPreview,
  McpLaunchAuthorizationResult,
  McpManagementErrorData,
  ManagedPlaywrightCancelNotification,
  ManagedPlaywrightCommandNotification,
  ManagedPlaywrightCompletionInput,
  BrowserRiskAuthorizeInput,
  BrowserRiskAuthorizeOutput,
  BrowserRiskCancelInput,
  McpServerCreateInput,
  McpServerDetailsOutput,
  McpServerIdInput,
  McpServerListOutput,
  McpServerMutationInput,
  McpServerUpdateInput,
  ProviderProfileUiDescriptor,
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
  SkillsInstallLocalInput,
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
  SkillsUpdateLocalInput,
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
  StorageModelSettingsRecord,
  StorageModelSettingsUpdateRecord,
  StorageProjectRecord,
  StorageUiPreferencesRecord
} from '@mycopilot/protocol'
import {
  AGENT_APPROVE_ACTION_METHOD,
  AGENT_CANCEL_ACTION_METHOD,
  AGENT_CANCEL_RUN_METHOD,
  AGENT_CLEAR_USAGE_RECORDS_METHOD,
  AGENT_COMMAND_SESSIONS_GET_METHOD,
  AGENT_COMMAND_SESSIONS_LIST_METHOD,
  AGENT_EVENT_NOTIFICATION_METHOD,
  AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD,
  AGENT_GET_FILE_WRITE_DIFF_METHOD,
  AGENT_GET_PROVIDER_TRANSITION_STATUS_METHOD,
  AGENT_GET_USAGE_SUMMARY_METHOD,
  AGENT_LIST_PENDING_ACTIONS_METHOD,
  AGENT_READ_FILE_DRAFT_METHOD,
  AGENT_REJECT_ACTION_METHOD,
  AGENT_PREFLIGHT_PROVIDER_TRANSITION_METHOD,
  AGENT_PROVIDER_TRANSITION_NOTIFICATION_METHOD,
  AGENT_START_PROVIDER_TRANSITION_METHOD,
  AGENT_START_CONVERSATION_TURN_METHOD,
  AGENT_STEER_RUN_METHOD,
  parseAgentActionExecutionOutputForHost,
  parseAgentCommandSessionGetInput,
  parseAgentCommandSessionGetOutput,
  parseAgentCommandSessionListInput,
  parseAgentCommandSessionListOutput,
  parseAgentEventForHost,
  parseAgentProviderTransitionNotification,
  parseAgentProviderTransitionOperation,
  parseAgentProviderTransitionPreflightInput,
  parseAgentProviderTransitionPreflightOutput,
  parseAgentProviderTransitionStartInput,
  parseAgentProviderTransitionStatusInput,
  parseAgentProviderTransitionStatusOutput,
  parsePendingAgentActionSnapshotsForHost,
  parseSkillInstallationCommitOutput,
  parseSkillInstallationPreview,
  parseSkillInspectionErrorData,
  parseSkillAcquisitionSource,
  parseSkillManagementErrorData,
  parseSkillMutationOutput,
  parseSkillPreparationCancellationOutput,
  parseSkillSourceResolutionErrorData,
  parseSkillsCancelSourceResolutionInput,
  parseSkillsCancelSourceResolutionOutput,
  parseSkillsChangedNotification,
  parseSkillsListManagementOutput,
  parseSkillsResolveInstallationSourceInput,
  parseSkillsResolveInstallationSourceOutput,
  parseSkillsSetEnabledOutput,
  IMAGE_GENERATION_CONFIGURATION_ERROR_CODE,
  IMAGE_GENERATION_GET_CONFIGURATION_METHOD,
  IMAGE_GENERATION_GET_STATUS_METHOD,
  IMAGE_GENERATION_SET_ENABLED_METHOD,
  IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD,
  MCP_CATALOG_REFRESH_METHOD,
  MCP_CATALOG_TOOLS_METHOD,
  MCP_BUILTIN_CAPABILITY_LIST_METHOD,
  MCP_BUILTIN_CAPABILITY_SET_ALLOWED_METHOD,
  MCP_CHANGED_NOTIFICATION_METHOD,
  MCP_MANAGEMENT_ERROR_CODE,
  MCP_MANAGEMENT_SCHEMA_VERSION,
  MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD,
  MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD,
  MANAGED_PLAYWRIGHT_COMPLETE_METHOD,
  BROWSER_RISK_AUTHORIZE_METHOD,
  BROWSER_RISK_CANCEL_METHOD,
  MCP_SERVER_ADD_METHOD,
  MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD,
  MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD,
  MCP_SERVER_DELETE_METHOD,
  MCP_SERVER_DISABLE_METHOD,
  MCP_SERVER_ENABLE_METHOD,
  MCP_SERVER_GET_METHOD,
  MCP_SERVER_LIST_METHOD,
  MCP_SERVER_RESTART_METHOD,
  MCP_SERVER_START_METHOD,
  MCP_SERVER_STATUS_METHOD,
  MCP_SERVER_STOP_METHOD,
  MCP_SERVER_UPDATE_METHOD,
  parseMcpCatalogToolsPageInput,
  parseMcpCatalogToolsPageOutput,
  parseMcpBuiltinCapabilityListOutput,
  parseMcpBuiltinCapabilityMutationOutput,
  parseMcpBuiltinCapabilitySetAllowedInput,
  parseMcpChangedNotification,
  parseMcpLaunchAuthorizationCommitInput,
  parseMcpLaunchAuthorizationPreview,
  parseMcpLaunchAuthorizationResult,
  parseMcpManagementErrorData,
  parseMcpServerCreateInput,
  parseMcpServerDetailsOutput,
  parseMcpServerIdInput,
  parseMcpServerListOutput,
  parseMcpServerMutationInput,
  parseMcpServerUpdateInput,
  parseManagedPlaywrightCancelNotification,
  parseManagedPlaywrightCommandNotification,
  parseManagedPlaywrightCompletionInput,
  parseManagedPlaywrightCompletionOutput,
  parseBrowserRiskAuthorizeInput,
  parseBrowserRiskAuthorizeOutput,
  parseBrowserRiskCancelInput,
  parseBrowserRiskCancelOutput,
  parseImageGenerationConfigurationErrorData,
  IMAGE_GENERATION_ARTIFACT_ERROR_CODE,
  IMAGE_GENERATION_READ_ARTIFACT_METHOD,
  parseImageGenerationArtifactErrorData,
  parseImageGenerationArtifactReadInput,
  parseImageGenerationArtifactReadOutput,
  parseImageGenerationGetConfigurationOutput,
  parseImageGenerationSetEnabledInput,
  parseImageGenerationSetEnabledOutput,
  parseImageGenerationStatus,
  parseImageGenerationUpdateConfigurationInput,
  parseImageGenerationUpdateConfigurationOutput,
  SKILLS_CANCEL_PREPARATION_METHOD,
  SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD,
  SKILLS_CHANGED_NOTIFICATION_METHOD,
  SKILLS_COMMIT_INSTALLATION_METHOD,
  SKILLS_INSTALL_LOCAL_METHOD,
  SKILLS_INSPECT_INSTALLATION_METHOD,
  SKILL_INSPECTION_ERROR_CODE,
  SKILL_MANAGEMENT_ERROR_CODE,
  SKILLS_LIST_MANAGEMENT_METHOD,
  SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD,
  SKILLS_SET_ENABLED_METHOD,
  SKILL_SOURCE_RESOLUTION_ERROR_CODE,
  OFFICE_GET_STATUS_METHOD,
  parseOfficeEngineStatus,
  parseAgentConversationLocator,
  parseAgentConversationLocatorRequest,
  parseAgentDetail,
  parseAgentDetailRequest,
  parseAgentObserverConversationRequest,
  parseAgentObserverConversation,
  parseAgentObserverEventEnvelope,
  parseAgentTemplate,
  parseAgentTemplateCreateRequest,
  parseAgentTemplateDeleteRequest,
  parseAgentTemplateList,
  parseAgentTemplateListRequest,
  parseAgentTemplateSetEnabledRequest,
  parseAgentTemplateUpdateRequest,
  parseAgentTreeRequest,
  parseAgentTreeLookup,
  parseCollaborationApprovalDecisionRequest,
  parseCollaborationApprovalDecisionResult,
  parseCollaborationApprovalList,
  parseCollaborationApprovalListRequest,
  parseCollaborationEventEnvelope,
  parseCollaborationEventsPage,
  parseCollaborationEventsRequest,
  parseCollaborationResyncEnvelope,
  SKILLS_UNINSTALL_METHOD,
  SKILLS_UPDATE_LOCAL_METHOD
} from '@mycopilot/protocol'

import { CoreJsonRpcClient } from './jsonRpcClient'

const CORE_PING_METHOD = 'core.ping'
const CORE_SHUTDOWN_METHOD = 'core.shutdown'
const SEARCH_SEARCH_CHATS_METHOD = 'search.searchChats'
const SKILLS_LIST_METHOD = 'skills.list'
const GIT_INSPECT_REPOSITORY_METHOD = 'git.inspectRepository'
const GIT_GET_REVIEW_SUMMARY_METHOD = 'git.getReviewSummary'
const GIT_GET_TURN_DIFF_SUMMARIES_METHOD = 'git.getTurnDiffSummaries'
const GIT_GET_REVIEW_FILE_DIFF_METHOD = 'git.getReviewFileDiff'
const GIT_GET_REVIEW_FILE_CONTENT_METHOD = 'git.getReviewFileContent'
const GIT_MUTATE_REVIEW_FILE_METHOD = 'git.mutateReviewFile'
const STORAGE_LOAD_MODEL_SETTINGS_METHOD = 'storage.loadModelSettings'
const STORAGE_LOAD_PROVIDER_PROFILE_UI_DESCRIPTORS_METHOD =
  'storage.loadProviderProfileUiDescriptors'
const STORAGE_SAVE_MODEL_SETTINGS_METHOD = 'storage.saveModelSettings'
const STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD = 'storage.loadAgentPromptPreferences'
const STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD = 'storage.saveAgentPromptPreferences'
const STORAGE_LOAD_PROJECTS_METHOD = 'storage.loadProjects'
const STORAGE_SAVE_PROJECT_METHOD = 'storage.saveProject'
const STORAGE_DELETE_PROJECT_METHOD = 'storage.deleteProject'
const STORAGE_LOAD_CONVERSATIONS_METHOD = 'storage.loadConversations'
const STORAGE_LOAD_CONVERSATION_METAS_METHOD = 'storage.loadConversationMetas'
const STORAGE_LOAD_CONVERSATION_METHOD = 'storage.loadConversation'
const STORAGE_SAVE_CONVERSATION_META_METHOD = 'storage.saveConversationMeta'
const STORAGE_DELETE_CONVERSATION_METHOD = 'storage.deleteConversation'
const STORAGE_DELETE_CHAT_MESSAGES_METHOD = 'storage.deleteChatMessages'
const STORAGE_FORK_CONVERSATION_METHOD = 'storage.forkConversation'
const STORAGE_UPSERT_CHAT_MESSAGES_METHOD = 'storage.upsertChatMessages'
const STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD = 'storage.saveChatMessageState'
const STORAGE_SAVE_CHAT_MESSAGE_UI_STATE_METHOD = 'storage.saveChatMessageUiState'
const STORAGE_LOAD_COMPOSER_DRAFTS_METHOD = 'storage.loadComposerDrafts'
const STORAGE_SAVE_COMPOSER_DRAFT_METHOD = 'storage.saveComposerDraft'
const STORAGE_LOAD_UI_PREFERENCES_METHOD = 'storage.loadUiPreferences'
const STORAGE_SAVE_UI_PREFERENCES_METHOD = 'storage.saveUiPreferences'
const STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD = 'storage.loadAttachmentImage'
const STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD = 'storage.loadInputAttachments'
const AGENT_REWRITE_CONVERSATION_TURN_METHOD = 'agent.rewriteConversationTurn'
const AGENT_COLLABORATION_GET_TREE_METHOD = 'agent.collaboration.getTree'
const AGENT_COLLABORATION_GET_AGENT_METHOD = 'agent.collaboration.getAgent'
const AGENT_COLLABORATION_LOCATE_CONVERSATION_METHOD = 'agent.collaboration.locateConversation'
const AGENT_COLLABORATION_LOAD_OBSERVER_CONVERSATION_METHOD =
  'agent.collaboration.loadObserverConversation'
const AGENT_COLLABORATION_LIST_EVENTS_METHOD = 'agent.collaboration.listEvents'
const AGENT_COLLABORATION_TEMPLATES_LIST_METHOD = 'agent.collaboration.templates.list'
const AGENT_COLLABORATION_TEMPLATES_CREATE_METHOD = 'agent.collaboration.templates.create'
const AGENT_COLLABORATION_TEMPLATES_UPDATE_METHOD = 'agent.collaboration.templates.update'
const AGENT_COLLABORATION_TEMPLATES_SET_ENABLED_METHOD = 'agent.collaboration.templates.setEnabled'
const AGENT_COLLABORATION_TEMPLATES_DELETE_METHOD = 'agent.collaboration.templates.delete'
const AGENT_COLLABORATION_APPROVALS_LIST_METHOD = 'agent.collaboration.approvals.list'
const AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD = 'agent.collaboration.approvals.decide'
const AGENT_COLLABORATION_EVENT_NOTIFICATION_METHOD = 'agent.collaboration.event'
const AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD = 'agent.collaboration.observerEvent'
const AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD = 'agent.collaboration.resync'

function validateProviderTransitionResponseIdentity(
  request: { conversationId: string; targetModelId: string },
  response: { conversationId: string; targetModelId: string }
): void {
  if (
    response.conversationId !== request.conversationId ||
    response.targetModelId !== request.targetModelId
  ) {
    throw new Error('Invalid Provider transition response identity')
  }
}

function rethrowValidatedSkillError<TData extends { message: string }>(
  error: unknown,
  code: number,
  parseData: (value: unknown) => TData
): never {
  if (
    typeof error !== 'object' ||
    error === null ||
    Array.isArray(error) ||
    !('code' in error) ||
    error.code !== code
  ) {
    throw error
  }

  const data = parseData('data' in error ? error.data : undefined)
  throw Object.assign(new Error(error instanceof Error ? error.message : data.message), {
    name: error instanceof Error ? error.name : 'Error',
    code,
    data
  })
}

function rethrowValidatedSkillInspectionError(error: unknown): never {
  return rethrowValidatedSkillError(
    error,
    SKILL_INSPECTION_ERROR_CODE,
    parseSkillInspectionErrorData
  )
}

function rethrowValidatedSkillManagementError(error: unknown): never {
  return rethrowValidatedSkillError(
    error,
    SKILL_MANAGEMENT_ERROR_CODE,
    parseSkillManagementErrorData
  )
}

function rethrowValidatedSkillSourceResolutionError(error: unknown): never {
  return rethrowValidatedSkillError(
    error,
    SKILL_SOURCE_RESOLUTION_ERROR_CODE,
    parseSkillSourceResolutionErrorData
  )
}

function rethrowValidatedImageGenerationConfigurationError(error: unknown): never {
  if (
    typeof error !== 'object' ||
    error === null ||
    Array.isArray(error) ||
    !('code' in error) ||
    error.code !== IMAGE_GENERATION_CONFIGURATION_ERROR_CODE
  ) {
    throw error
  }

  const data = parseImageGenerationConfigurationErrorData('data' in error ? error.data : undefined)
  // The validated, bounded domain message is authoritative. A provider or transport error message
  // must never be forwarded because it could contain a URL, response body, or credential material.
  throw Object.assign(new Error(data.message), {
    name: 'ImageGenerationConfigurationError',
    code: IMAGE_GENERATION_CONFIGURATION_ERROR_CODE,
    data
  })
}

function rethrowValidatedImageGenerationArtifactError(error: unknown): never {
  if (
    typeof error !== 'object' ||
    error === null ||
    Array.isArray(error) ||
    !('code' in error) ||
    error.code !== IMAGE_GENERATION_ARTIFACT_ERROR_CODE
  ) {
    throw error
  }

  const data = parseImageGenerationArtifactErrorData('data' in error ? error.data : undefined)
  throw Object.assign(new Error(data.message), {
    name: 'ImageGenerationArtifactError',
    code: IMAGE_GENERATION_ARTIFACT_ERROR_CODE,
    data
  })
}

function rethrowValidatedMcpManagementError(error: unknown): never {
  if (
    typeof error !== 'object' ||
    error === null ||
    Array.isArray(error) ||
    !('code' in error) ||
    error.code !== MCP_MANAGEMENT_ERROR_CODE
  ) {
    throw error
  }

  const data: McpManagementErrorData = parseMcpManagementErrorData(
    'data' in error ? error.data : undefined
  )
  // Only the bounded, protocol-validated Host projection may cross Main. Core/Server messages can
  // contain process details and are deliberately not forwarded.
  throw Object.assign(new Error(data.message), {
    name: 'McpManagementError',
    code: MCP_MANAGEMENT_ERROR_CODE,
    data
  })
}

function validatedImageGenerationArtifactContent(
  output: ImageGenerationArtifactReadOutput,
  expected: ManagedArtifactReadIdentity
): ImageGenerationArtifactContent {
  if (!sameManagedArtifact(output.artifact, expected)) {
    throw new Error('Invalid Image generation Artifact read response: frozen identity changed')
  }
  const bytes = Buffer.from(output.dataBase64, 'base64')
  if (
    bytes.byteLength !== output.artifact.sizeBytes ||
    bytes.toString('base64') !== output.dataBase64
  ) {
    throw new Error('Invalid Image generation Artifact read response: byte length changed')
  }
  const sha256 = createHash('sha256').update(bytes).digest('hex')
  if (sha256 !== output.artifact.sha256) {
    throw new Error('Invalid Image generation Artifact read response: content digest changed')
  }
  return {
    schemaVersion: output.schemaVersion,
    artifact: output.artifact,
    fileName: output.fileName,
    bytes: Uint8Array.from(bytes)
  }
}

function sameManagedArtifact(
  left: ManagedArtifactReadIdentity,
  right: ManagedArtifactReadIdentity
): boolean {
  if (
    left.artifactId !== right.artifactId ||
    left.uri !== right.uri ||
    left.kind !== right.kind ||
    left.format !== right.format ||
    left.mimeType !== right.mimeType ||
    left.sizeBytes !== right.sizeBytes ||
    left.sha256 !== right.sha256
  ) {
    return false
  }
  if (left.kind === 'image' && right.kind === 'image') {
    return left.width === right.width && left.height === right.height
  }
  return left.kind === 'document' && right.kind === 'document'
}

export interface CoreServerOptions {
  /**
   * The Electron Host-owned application data root. Production construction must inject the
   * value frozen from app.getPath('userData'); the optional fallback keeps isolated unit-test
   * construction and non-entrypoint consumers source-compatible.
   */
  appDataRoot?: string
}

export class CoreServer {
  private readonly rpc: CoreJsonRpcClient

  constructor(options: CoreServerOptions = {}) {
    this.rpc =
      options.appDataRoot === undefined
        ? new CoreJsonRpcClient()
        : new CoreJsonRpcClient({ appDataRoot: options.appDataRoot })
  }

  start(): void {
    this.rpc.start()
  }

  stop(): void {
    this.rpc.stop()
  }

  async shutdown(): Promise<void> {
    if (!this.rpc.isRunning()) return

    // Core first gives the managed Playwright bridge up to two seconds to settle, then shuts down
    // the remaining Agent/MCP services in parallel under their own two-second bounds. Keep the
    // Host watchdog larger than that composed budget so it does not kill Core in the middle of
    // external MCP or Agent cleanup. This remains a hard upper bound for application exit.
    const hostShutdownTimeoutMs = 6_000
    let timeoutId: ReturnType<typeof setTimeout> | null = null
    const timeout = new Promise<void>((resolve) => {
      timeoutId = setTimeout(resolve, hostShutdownTimeoutMs)
      timeoutId.unref()
    })
    const shutdown = this.rpc
      .request<CoreShutdownResponse>(CORE_SHUTDOWN_METHOD)
      .then((response) => {
        if (response.timedOut) {
          console.warn('core-server shutdown timed out while waiting for active agent runs')
        }
      })
      .catch((error) => {
        console.warn('Failed to request core-server shutdown', error)
      })

    await Promise.race([shutdown, timeout])
    if (timeoutId) clearTimeout(timeoutId)
    this.rpc.stop()
  }

  ping(input?: CorePingRequest): Promise<CorePingResponse> {
    return this.rpc.request<CorePingResponse, CorePingRequest>(CORE_PING_METHOD, input ?? {})
  }

  getOfficeStatus(): Promise<OfficeEngineStatus> {
    return this.rpc.request<unknown>(OFFICE_GET_STATUS_METHOD).then(parseOfficeEngineStatus)
  }

  getImageGenerationConfiguration(): Promise<ImageGenerationGetConfigurationOutput> {
    return this.rpc
      .request<unknown>(IMAGE_GENERATION_GET_CONFIGURATION_METHOD)
      .then(parseImageGenerationGetConfigurationOutput)
      .catch(rethrowValidatedImageGenerationConfigurationError)
  }

  updateImageGenerationConfiguration(
    input: ImageGenerationUpdateConfigurationInput
  ): Promise<ImageGenerationUpdateConfigurationOutput> {
    const request = parseImageGenerationUpdateConfigurationInput(input)
    return this.rpc
      .request<unknown, ImageGenerationUpdateConfigurationInput>(
        IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD,
        request
      )
      .then(parseImageGenerationUpdateConfigurationOutput)
      .catch(rethrowValidatedImageGenerationConfigurationError)
  }

  setImageGenerationEnabled(
    input: ImageGenerationSetEnabledInput
  ): Promise<ImageGenerationSetEnabledOutput> {
    const request = parseImageGenerationSetEnabledInput(input)
    return this.rpc
      .request<unknown, ImageGenerationSetEnabledInput>(
        IMAGE_GENERATION_SET_ENABLED_METHOD,
        request
      )
      .then(parseImageGenerationSetEnabledOutput)
      .catch(rethrowValidatedImageGenerationConfigurationError)
  }

  getImageGenerationStatus(): Promise<ImageGenerationStatus> {
    return this.rpc
      .request<unknown>(IMAGE_GENERATION_GET_STATUS_METHOD)
      .then(parseImageGenerationStatus)
      .catch(rethrowValidatedImageGenerationConfigurationError)
  }

  readImageGenerationArtifact(
    input: ImageGenerationArtifactReadInput
  ): Promise<ImageGenerationArtifactContent> {
    const request = parseImageGenerationArtifactReadInput(input)
    return this.rpc
      .request<unknown, ImageGenerationArtifactReadInput>(
        IMAGE_GENERATION_READ_ARTIFACT_METHOD,
        request
      )
      .then(parseImageGenerationArtifactReadOutput)
      .then((output) => validatedImageGenerationArtifactContent(output, request.artifact))
      .catch(rethrowValidatedImageGenerationArtifactError)
  }

  listMcpServers(): Promise<McpServerListOutput> {
    return this.rpc
      .request<unknown, { schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION }>(
        MCP_SERVER_LIST_METHOD,
        { schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION }
      )
      .then(parseMcpServerListOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  listMcpBuiltinCapabilities(): Promise<McpBuiltinCapabilityListOutput> {
    return this.rpc
      .request<unknown, { schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION }>(
        MCP_BUILTIN_CAPABILITY_LIST_METHOD,
        { schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION }
      )
      .then(parseMcpBuiltinCapabilityListOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  setMcpBuiltinCapabilityAllowed(
    input: McpBuiltinCapabilitySetAllowedInput
  ): Promise<McpBuiltinCapabilityMutationOutput> {
    const request = parseMcpBuiltinCapabilitySetAllowedInput(input)
    return this.rpc
      .request<unknown, McpBuiltinCapabilitySetAllowedInput>(
        MCP_BUILTIN_CAPABILITY_SET_ALLOWED_METHOD,
        request
      )
      .then(parseMcpBuiltinCapabilityMutationOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  getMcpServer(input: McpServerIdInput): Promise<McpServerDetailsOutput> {
    const request = parseMcpServerIdInput(input)
    return this.rpc
      .request<unknown, McpServerIdInput>(MCP_SERVER_GET_METHOD, request)
      .then(parseMcpServerDetailsOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  addMcpServer(input: McpServerCreateInput): Promise<McpServerDetailsOutput> {
    const request = parseMcpServerCreateInput(input)
    return this.rpc
      .request<unknown, McpServerCreateInput>(MCP_SERVER_ADD_METHOD, request)
      .then(parseMcpServerDetailsOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  updateMcpServer(input: McpServerUpdateInput): Promise<McpServerDetailsOutput> {
    const request = parseMcpServerUpdateInput(input)
    return this.rpc
      .request<unknown, McpServerUpdateInput>(MCP_SERVER_UPDATE_METHOD, request)
      .then(parseMcpServerDetailsOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  deleteMcpServer(input: McpServerMutationInput): Promise<McpServerDetailsOutput> {
    const request = parseMcpServerMutationInput(input)
    return this.rpc
      .request<unknown, McpServerMutationInput>(MCP_SERVER_DELETE_METHOD, request)
      .then(parseMcpServerDetailsOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  prepareMcpLaunchAuthorization(
    input: McpServerMutationInput
  ): Promise<McpLaunchAuthorizationPreview> {
    const request = parseMcpServerMutationInput(input)
    return this.rpc
      .request<unknown, McpServerMutationInput>(MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD, request)
      .then(parseMcpLaunchAuthorizationPreview)
      .catch(rethrowValidatedMcpManagementError)
  }

  commitMcpLaunchAuthorization(
    input: McpLaunchAuthorizationCommitInput
  ): Promise<McpLaunchAuthorizationResult> {
    const request = parseMcpLaunchAuthorizationCommitInput(input)
    return this.rpc
      .request<unknown, McpLaunchAuthorizationCommitInput>(
        MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD,
        request
      )
      .then(parseMcpLaunchAuthorizationResult)
      .catch(rethrowValidatedMcpManagementError)
  }

  enableMcpServer(input: McpServerMutationInput): Promise<McpServerDetailsOutput> {
    return this.mutateMcpServer(MCP_SERVER_ENABLE_METHOD, input)
  }

  disableMcpServer(input: McpServerMutationInput): Promise<McpServerDetailsOutput> {
    return this.mutateMcpServer(MCP_SERVER_DISABLE_METHOD, input)
  }

  startMcpServer(input: McpServerMutationInput): Promise<McpServerDetailsOutput> {
    return this.mutateMcpServer(MCP_SERVER_START_METHOD, input)
  }

  stopMcpServer(input: McpServerMutationInput): Promise<McpServerDetailsOutput> {
    return this.mutateMcpServer(MCP_SERVER_STOP_METHOD, input)
  }

  restartMcpServer(input: McpServerMutationInput): Promise<McpServerDetailsOutput> {
    return this.mutateMcpServer(MCP_SERVER_RESTART_METHOD, input)
  }

  getMcpServerStatus(input: McpServerIdInput): Promise<McpServerDetailsOutput> {
    const request = parseMcpServerIdInput(input)
    return this.rpc
      .request<unknown, McpServerIdInput>(MCP_SERVER_STATUS_METHOD, request)
      .then(parseMcpServerDetailsOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  listMcpTools(input: McpCatalogToolsPageInput): Promise<McpCatalogToolsPageOutput> {
    const request = parseMcpCatalogToolsPageInput(input)
    return this.rpc
      .request<unknown, McpCatalogToolsPageInput>(MCP_CATALOG_TOOLS_METHOD, request)
      .then(parseMcpCatalogToolsPageOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  refreshMcpCatalog(input: McpServerMutationInput): Promise<McpCatalogToolsPageOutput> {
    const request = parseMcpServerMutationInput(input)
    return this.rpc
      .request<unknown, McpServerMutationInput>(MCP_CATALOG_REFRESH_METHOD, request)
      .then(parseMcpCatalogToolsPageOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  onMcpChanged(handler: (event: McpChangedNotification) => void): () => void {
    return this.rpc.onNotification(MCP_CHANGED_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseMcpChangedNotification(params))
      } catch {
        // Do not log protocol bodies or parser errors: either may contain rejected private data.
        console.warn('Ignored invalid mcp.changed notification')
      }
    })
  }

  onManagedPlaywrightCommand(
    handler: (event: ManagedPlaywrightCommandNotification) => void
  ): () => void {
    return this.rpc.onNotification(MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseManagedPlaywrightCommandNotification(params))
      } catch {
        console.warn('Ignored invalid managed Playwright command notification')
      }
    })
  }

  onManagedPlaywrightCancel(
    handler: (event: ManagedPlaywrightCancelNotification) => void
  ): () => void {
    return this.rpc.onNotification(MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseManagedPlaywrightCancelNotification(params))
      } catch {
        console.warn('Ignored invalid managed Playwright cancel notification')
      }
    })
  }

  completeManagedPlaywright(input: ManagedPlaywrightCompletionInput): Promise<void> {
    const request = parseManagedPlaywrightCompletionInput(input)
    return this.rpc
      .request<unknown, ManagedPlaywrightCompletionInput>(
        MANAGED_PLAYWRIGHT_COMPLETE_METHOD,
        request
      )
      .then(parseManagedPlaywrightCompletionOutput)
      .then((output) => {
        if (!output.accepted) throw new Error('Managed Playwright completion was not accepted')
      })
  }

  authorizeBrowserRisk(input: BrowserRiskAuthorizeInput): Promise<BrowserRiskAuthorizeOutput> {
    const request = parseBrowserRiskAuthorizeInput(input)
    return this.rpc
      .request<unknown, BrowserRiskAuthorizeInput>(BROWSER_RISK_AUTHORIZE_METHOD, request)
      .then(parseBrowserRiskAuthorizeOutput)
  }

  cancelBrowserRisk(input: BrowserRiskCancelInput): Promise<boolean> {
    const request = parseBrowserRiskCancelInput(input)
    return this.rpc
      .request<unknown, BrowserRiskCancelInput>(BROWSER_RISK_CANCEL_METHOD, request)
      .then(parseBrowserRiskCancelOutput)
      .then((output) => output.accepted)
  }

  private mutateMcpServer(
    method:
      | typeof MCP_SERVER_ENABLE_METHOD
      | typeof MCP_SERVER_DISABLE_METHOD
      | typeof MCP_SERVER_START_METHOD
      | typeof MCP_SERVER_STOP_METHOD
      | typeof MCP_SERVER_RESTART_METHOD,
    input: McpServerMutationInput
  ): Promise<McpServerDetailsOutput> {
    const request = parseMcpServerMutationInput(input)
    return this.rpc
      .request<unknown, McpServerMutationInput>(method, request)
      .then(parseMcpServerDetailsOutput)
      .catch(rethrowValidatedMcpManagementError)
  }

  preflightProviderTransition(
    input: AgentProviderTransitionPreflightInput
  ): Promise<AgentProviderTransitionPreflightOutput> {
    const request = parseAgentProviderTransitionPreflightInput(input)
    return this.rpc
      .request<unknown, AgentProviderTransitionPreflightInput>(
        AGENT_PREFLIGHT_PROVIDER_TRANSITION_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentProviderTransitionPreflightOutput(value)
        validateProviderTransitionResponseIdentity(request, output)
        return output
      })
      .catch(() => {
        throw new Error('Unable to check this model switch. Please try again.')
      })
  }

  startProviderTransition(
    input: AgentProviderTransitionStartInput
  ): Promise<AgentProviderTransitionOperation> {
    const request = parseAgentProviderTransitionStartInput(input)
    return this.rpc
      .request<unknown, AgentProviderTransitionStartInput>(
        AGENT_START_PROVIDER_TRANSITION_METHOD,
        request
      )
      .then((value) => {
        const operation = parseAgentProviderTransitionOperation(value)
        validateProviderTransitionResponseIdentity(request, operation)
        return operation
      })
      .catch(() => {
        throw new Error('The model-switch check expired. Please try again.')
      })
  }

  getProviderTransitionStatus(
    input: AgentProviderTransitionStatusInput
  ): Promise<AgentProviderTransitionStatusOutput> {
    const request = parseAgentProviderTransitionStatusInput(input)
    return this.rpc
      .request<unknown, AgentProviderTransitionStatusInput>(
        AGENT_GET_PROVIDER_TRANSITION_STATUS_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentProviderTransitionStatusOutput(value)
        if (
          output.operations.some(
            (operation) =>
              operation.conversationId !== request.conversationId ||
              (request.operationId !== undefined && operation.operationId !== request.operationId)
          )
        ) {
          throw new Error('Invalid Provider transition status response identity')
        }
        return output
      })
      .catch(() => {
        throw new Error('Unable to restore the model-switch status. Please try again.')
      })
  }

  onProviderTransition(handler: (event: AgentProviderTransitionNotification) => void): () => void {
    return this.rpc.onNotification(AGENT_PROVIDER_TRANSITION_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseAgentProviderTransitionNotification(params))
      } catch {
        // Do not echo rejected transition payloads: they may contain future private fields.
        console.warn('Ignored invalid Provider transition notification')
      }
    })
  }

  startConversationTurn(input: AgentConversationTurnInput): Promise<AgentConversationTurnOutput> {
    return this.rpc.request<AgentConversationTurnOutput, AgentConversationTurnInput>(
      AGENT_START_CONVERSATION_TURN_METHOD,
      input
    )
  }

  rewriteConversationTurn(
    input: AgentConversationTurnRewriteInput
  ): Promise<AgentConversationTurnOutput> {
    return this.rpc.request<AgentConversationTurnOutput, AgentConversationTurnRewriteInput>(
      AGENT_REWRITE_CONVERSATION_TURN_METHOD,
      input
    )
  }

  getContextWindowSnapshot(
    input: AgentContextWindowSnapshotInput
  ): Promise<AgentContextWindowSnapshotOutput> {
    return this.rpc.request<AgentContextWindowSnapshotOutput, AgentContextWindowSnapshotInput>(
      AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD,
      input
    )
  }

  listCommandSessions(input: AgentCommandSessionListInput): Promise<AgentCommandSessionListOutput> {
    const request = parseAgentCommandSessionListInput(input)
    return this.rpc
      .request<unknown, AgentCommandSessionListInput>(AGENT_COMMAND_SESSIONS_LIST_METHOD, request)
      .then(parseAgentCommandSessionListOutput)
  }

  getCommandSession(input: AgentCommandSessionGetInput): Promise<AgentCommandSessionGetOutput> {
    const request = parseAgentCommandSessionGetInput(input)
    return this.rpc
      .request<unknown, AgentCommandSessionGetInput>(AGENT_COMMAND_SESSIONS_GET_METHOD, request)
      .then(parseAgentCommandSessionGetOutput)
  }

  cancelRun(input: AgentCancelRunRequest): Promise<AgentCancelRunResponse> {
    return this.rpc.request<AgentCancelRunResponse, AgentCancelRunRequest>(
      AGENT_CANCEL_RUN_METHOD,
      input
    )
  }

  steerRun(input: AgentSteerRunInput): Promise<AgentSteerRunOutput> {
    return this.rpc.request<AgentSteerRunOutput, AgentSteerRunInput>(AGENT_STEER_RUN_METHOD, input)
  }

  listPendingActions(): Promise<PendingAgentActionSnapshot[]> {
    return this.rpc
      .request<unknown>(AGENT_LIST_PENDING_ACTIONS_METHOD)
      .then(parsePendingAgentActionSnapshotsForHost)
  }

  approveAction(input: AgentActionIdRequest): Promise<AgentActionExecutionOutput> {
    return this.rpc
      .request<unknown, AgentActionIdRequest>(AGENT_APPROVE_ACTION_METHOD, input)
      .then(parseAgentActionExecutionOutputForHost)
  }

  rejectAction(input: AgentRejectActionRequest): Promise<AgentActionExecutionOutput> {
    return this.rpc
      .request<unknown, AgentRejectActionRequest>(AGENT_REJECT_ACTION_METHOD, input)
      .then(parseAgentActionExecutionOutputForHost)
  }

  cancelAction(input: AgentActionIdRequest): Promise<boolean> {
    return this.rpc.request<boolean, AgentActionIdRequest>(AGENT_CANCEL_ACTION_METHOD, input)
  }

  getUsageSummary(input: AgentUsageSummaryInput): Promise<AgentUsageSummaryOutput> {
    return this.rpc.request<AgentUsageSummaryOutput, AgentUsageSummaryInput>(
      AGENT_GET_USAGE_SUMMARY_METHOD,
      input
    )
  }

  clearUsageRecords(input: AgentUsageClearInput): Promise<AgentUsageClearOutput> {
    return this.rpc.request<AgentUsageClearOutput, AgentUsageClearInput>(
      AGENT_CLEAR_USAGE_RECORDS_METHOD,
      input
    )
  }

  readFileDraft(input: AgentFileDraftReadInput): Promise<AgentFileDraftContentPage> {
    return this.rpc.request<AgentFileDraftContentPage, AgentFileDraftReadInput>(
      AGENT_READ_FILE_DRAFT_METHOD,
      input
    )
  }

  getFileWriteDiff(input: AgentFileWriteDiffInput): Promise<AgentFileWriteDiffPage> {
    return this.rpc.request<AgentFileWriteDiffPage, AgentFileWriteDiffInput>(
      AGENT_GET_FILE_WRITE_DIFF_METHOD,
      input
    )
  }

  onAgentEvent(handler: (event: AgentEvent) => void): () => void {
    return this.rpc.onNotification(AGENT_EVENT_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseAgentEventForHost(params))
      } catch {
        // MCP event rejection must not echo the rejected payload or a parser diagnostic.
        console.warn('Ignored invalid Agent event')
      }
    })
  }

  getCollaborationTree(input: AgentTreeRequest): Promise<AgentTreeLookup> {
    const request = parseAgentTreeRequest(input)
    return this.rpc
      .request<unknown, AgentTreeRequest>(AGENT_COLLABORATION_GET_TREE_METHOD, request)
      .then((value) => {
        const lookup = parseAgentTreeLookup(value)
        const tree = lookup.tree
        if (!tree) return lookup
        if (
          tree.rootConversationId !== request.rootConversationId ||
          tree.agents.some(
            (agent) =>
              agent.rootConversationId !== tree.rootConversationId ||
              agent.rootAgentId !== tree.rootAgentId ||
              agent.projectId !== tree.projectId
          )
        ) {
          throw new Error('Invalid collaboration tree response identity')
        }
        return lookup
      })
  }

  getCollaborationAgent(input: AgentDetailRequest): Promise<AgentDetail> {
    const request = parseAgentDetailRequest(input)
    return this.rpc
      .request<unknown, AgentDetailRequest>(AGENT_COLLABORATION_GET_AGENT_METHOD, request)
      .then((value) => {
        const detail = parseAgentDetail(value)
        if (
          detail.summary.agentId !== request.agentId ||
          detail.summary.rootConversationId !== request.rootConversationId
        ) {
          throw new Error('Invalid collaboration Agent response identity')
        }
        return detail
      })
  }

  locateCollaborationConversation(
    input: AgentConversationLocatorRequest
  ): Promise<AgentConversationLocator> {
    const request = parseAgentConversationLocatorRequest(input)
    return this.rpc
      .request<unknown, AgentConversationLocatorRequest>(
        AGENT_COLLABORATION_LOCATE_CONVERSATION_METHOD,
        request
      )
      .then((value) => {
        const locator = parseAgentConversationLocator(value)
        if (locator.agentId !== request.agentId) {
          throw new Error('Invalid collaboration locator response identity')
        }
        return locator
      })
  }

  loadCollaborationObserverConversation(
    input: AgentObserverConversationRequest
  ): Promise<AgentObserverConversation | null> {
    const request = parseAgentObserverConversationRequest(input)
    return this.rpc
      .request<unknown, AgentObserverConversationRequest>(
        AGENT_COLLABORATION_LOAD_OBSERVER_CONVERSATION_METHOD,
        request
      )
      .then((value) => {
        const response = parseAgentObserverConversation(value)
        if (
          response &&
          (response.rootConversationId !== request.rootConversationId ||
            response.conversationId !== request.conversationId)
        ) {
          throw new Error('Invalid observer Conversation response identity')
        }
        return response
      })
  }

  listCollaborationEvents(input: CollaborationEventsRequest): Promise<CollaborationEventsPage> {
    const request = parseCollaborationEventsRequest(input)
    return this.rpc
      .request<unknown, CollaborationEventsRequest>(AGENT_COLLABORATION_LIST_EVENTS_METHOD, request)
      .then((value) => {
        const page = parseCollaborationEventsPage(value)
        if (
          page.rootConversationId !== request.rootConversationId ||
          page.events.some(
            (event) =>
              event.rootConversationId !== page.rootConversationId ||
              event.rootAgentId !== page.rootAgentId
          )
        ) {
          throw new Error('Invalid collaboration event page identity')
        }
        return page
      })
  }

  listAgentTemplates(input: AgentTemplateListRequest): Promise<AgentTemplateList> {
    const request = parseAgentTemplateListRequest(input)
    return this.rpc
      .request<unknown, AgentTemplateListRequest>(
        AGENT_COLLABORATION_TEMPLATES_LIST_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentTemplateList(value)
        if (output.templates.some((template) => template.projectId !== request.projectId)) {
          throw new Error('Invalid Agent template list identity')
        }
        return output
      })
  }

  createAgentTemplate(input: AgentTemplateCreateRequest): Promise<AgentTemplate> {
    const request = parseAgentTemplateCreateRequest(input)
    return this.rpc
      .request<unknown, AgentTemplateCreateRequest>(
        AGENT_COLLABORATION_TEMPLATES_CREATE_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentTemplate(value)
        if (output.projectId !== request.projectId || output.templateId !== request.templateId) {
          throw new Error('Invalid Agent template response identity')
        }
        return output
      })
  }

  updateAgentTemplate(input: AgentTemplateUpdateRequest): Promise<AgentTemplate> {
    const request = parseAgentTemplateUpdateRequest(input)
    return this.rpc
      .request<unknown, AgentTemplateUpdateRequest>(
        AGENT_COLLABORATION_TEMPLATES_UPDATE_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentTemplate(value)
        if (output.projectId !== request.projectId || output.templateId !== request.templateId) {
          throw new Error('Invalid Agent template response identity')
        }
        return output
      })
  }

  setAgentTemplateEnabled(input: AgentTemplateSetEnabledRequest): Promise<AgentTemplate> {
    const request = parseAgentTemplateSetEnabledRequest(input)
    return this.rpc
      .request<unknown, AgentTemplateSetEnabledRequest>(
        AGENT_COLLABORATION_TEMPLATES_SET_ENABLED_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentTemplate(value)
        if (output.projectId !== request.projectId || output.templateId !== request.templateId) {
          throw new Error('Invalid Agent template response identity')
        }
        return output
      })
  }

  deleteAgentTemplate(input: AgentTemplateDeleteRequest): Promise<AgentTemplate> {
    const request = parseAgentTemplateDeleteRequest(input)
    return this.rpc
      .request<unknown, AgentTemplateDeleteRequest>(
        AGENT_COLLABORATION_TEMPLATES_DELETE_METHOD,
        request
      )
      .then((value) => {
        const output = parseAgentTemplate(value)
        if (output.projectId !== request.projectId || output.templateId !== request.templateId) {
          throw new Error('Invalid Agent template response identity')
        }
        return output
      })
  }

  listCollaborationApprovals(
    input: CollaborationApprovalListRequest
  ): Promise<CollaborationApprovalList> {
    const request = parseCollaborationApprovalListRequest(input)
    return this.rpc
      .request<unknown, CollaborationApprovalListRequest>(
        AGENT_COLLABORATION_APPROVALS_LIST_METHOD,
        request
      )
      .then((value) => {
        const output = parseCollaborationApprovalList(value)
        if (
          output.approvals.some(
            (approval) => approval.rootConversationId !== request.rootConversationId
          )
        ) {
          throw new Error('Invalid collaboration Approval list identity')
        }
        return output
      })
  }

  decideCollaborationApproval(
    input: CollaborationApprovalDecisionRequest
  ): Promise<CollaborationApprovalDecisionResult> {
    const request = parseCollaborationApprovalDecisionRequest(input)
    return this.rpc
      .request<unknown, CollaborationApprovalDecisionRequest>(
        AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD,
        request
      )
      .then((value) => {
        const output = parseCollaborationApprovalDecisionResult(value)
        if (output.approvalId !== request.approvalId) {
          throw new Error('Invalid collaboration Approval response identity')
        }
        return output
      })
  }

  onCollaborationEvent(handler: (event: CollaborationEventEnvelope) => void): () => void {
    return this.rpc.onNotification(AGENT_COLLABORATION_EVENT_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseCollaborationEventEnvelope(params))
      } catch {
        console.warn('Ignored invalid collaboration event')
      }
    })
  }

  onCollaborationObserverEvent(handler: (event: AgentObserverEventEnvelope) => void): () => void {
    return this.rpc.onNotification(
      AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD,
      (params) => {
        try {
          handler(parseAgentObserverEventEnvelope(params))
        } catch {
          console.warn('Ignored invalid Agent observer event')
        }
      }
    )
  }

  onCollaborationResync(handler: (event: CollaborationResyncEnvelope) => void): () => void {
    return this.rpc.onNotification(AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseCollaborationResyncEnvelope(params))
      } catch {
        console.warn('Ignored invalid collaboration resync')
      }
    })
  }

  searchChats(input: ChatSearchInput): Promise<ChatSearchResult[]> {
    return this.rpc.request<ChatSearchResult[], ChatSearchInput>(SEARCH_SEARCH_CHATS_METHOD, input)
  }

  listSkills(input: SkillsListInput): Promise<SkillsListOutput> {
    return this.rpc.request<SkillsListOutput, SkillsListInput>(SKILLS_LIST_METHOD, input)
  }

  resolveSkillInstallationSource(
    input: SkillsResolveInstallationSourceInput
  ): Promise<SkillsResolveInstallationSourceOutput> {
    const request = parseSkillsResolveInstallationSourceInput(input)
    return this.rpc
      .request<unknown, SkillsResolveInstallationSourceInput>(
        SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD,
        request
      )
      .then((value) => {
        const response = parseSkillsResolveInstallationSourceOutput(value)
        if (response.resolutionId !== request.resolutionId) {
          throw new Error(
            'Invalid Skill source resolution response: resolutionId must match request.resolutionId'
          )
        }
        return response
      })
      .catch(rethrowValidatedSkillSourceResolutionError)
  }

  cancelSkillSourceResolution(
    input: SkillsCancelSourceResolutionInput
  ): Promise<SkillsCancelSourceResolutionOutput> {
    const request = parseSkillsCancelSourceResolutionInput(input)
    return this.rpc
      .request<unknown, SkillsCancelSourceResolutionInput>(
        SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD,
        request
      )
      .then((value) => {
        const response = parseSkillsCancelSourceResolutionOutput(value)
        if (response.resolutionId !== request.resolutionId) {
          throw new Error(
            'Invalid Skill source resolution cancellation response: resolutionId must match request.resolutionId'
          )
        }
        return response
      })
      .catch(rethrowValidatedSkillSourceResolutionError)
  }

  inspectSkillInstallation(
    input: SkillsInspectInstallationInput
  ): Promise<SkillInstallationPreview> {
    const request = { ...input, source: parseSkillAcquisitionSource(input.source) }
    return this.rpc
      .request<unknown, SkillsInspectInstallationInput>(SKILLS_INSPECT_INSTALLATION_METHOD, request)
      .then(parseSkillInstallationPreview)
      .catch(rethrowValidatedSkillInspectionError)
  }

  commitSkillInstallation(
    input: SkillsCommitInstallationInput
  ): Promise<SkillInstallationCommitOutput> {
    return this.rpc
      .request<unknown, SkillsCommitInstallationInput>(SKILLS_COMMIT_INSTALLATION_METHOD, input)
      .then(parseSkillInstallationCommitOutput)
      .catch(rethrowValidatedSkillInspectionError)
  }

  cancelSkillPreparation(
    input: SkillsCancelPreparationInput
  ): Promise<SkillPreparationCancellationOutput> {
    return this.rpc
      .request<unknown, SkillsCancelPreparationInput>(SKILLS_CANCEL_PREPARATION_METHOD, input)
      .then(parseSkillPreparationCancellationOutput)
      .catch(rethrowValidatedSkillInspectionError)
  }

  listSkillManagement(input: SkillsListManagementInput): Promise<SkillsListManagementOutput> {
    return this.rpc
      .request<unknown, SkillsListManagementInput>(SKILLS_LIST_MANAGEMENT_METHOD, input)
      .then(parseSkillsListManagementOutput)
      .catch(rethrowValidatedSkillManagementError)
  }

  setSkillEnabled(input: SkillsSetEnabledInput): Promise<SkillsSetEnabledOutput> {
    return this.rpc
      .request<unknown, SkillsSetEnabledInput>(SKILLS_SET_ENABLED_METHOD, input)
      .then(parseSkillsSetEnabledOutput)
      .catch(rethrowValidatedSkillManagementError)
  }

  onSkillsChanged(handler: (event: SkillsChangedNotification) => void): () => void {
    return this.rpc.onNotification(SKILLS_CHANGED_NOTIFICATION_METHOD, (params) => {
      try {
        handler(parseSkillsChangedNotification(params))
      } catch (error) {
        console.warn('Ignored invalid skills.changed notification', error)
      }
    })
  }

  installLocalSkill(input: SkillsInstallLocalInput): Promise<SkillMutationOutput> {
    return this.rpc
      .request<unknown, SkillsInstallLocalInput>(SKILLS_INSTALL_LOCAL_METHOD, input)
      .then((value) => parseSkillMutationOutput(value, 'install'))
  }

  updateLocalSkill(input: SkillsUpdateLocalInput): Promise<SkillMutationOutput> {
    return this.rpc
      .request<unknown, SkillsUpdateLocalInput>(SKILLS_UPDATE_LOCAL_METHOD, input)
      .then((value) => parseSkillMutationOutput(value, 'update'))
  }

  uninstallSkill(input: SkillsUninstallInput): Promise<SkillMutationOutput> {
    return this.rpc
      .request<unknown, SkillsUninstallInput>(SKILLS_UNINSTALL_METHOD, input)
      .then((value) => parseSkillMutationOutput(value, 'uninstall'))
  }

  inspectGitRepository(input: GitRepositoryInspectInput): Promise<GitRepositoryInspection> {
    return this.rpc.request<GitRepositoryInspection, GitRepositoryInspectInput>(
      GIT_INSPECT_REPOSITORY_METHOD,
      input
    )
  }

  getGitReviewSummary(input: GitReviewSummaryInput): Promise<GitReviewSummary> {
    return this.rpc.request<GitReviewSummary, GitReviewSummaryInput>(
      GIT_GET_REVIEW_SUMMARY_METHOD,
      input
    )
  }

  getGitTurnDiffSummaries(input: GitTurnDiffSummariesInput): Promise<GitTurnDiffSummaries> {
    return this.rpc.request<GitTurnDiffSummaries, GitTurnDiffSummariesInput>(
      GIT_GET_TURN_DIFF_SUMMARIES_METHOD,
      input
    )
  }

  getGitReviewFileDiff(input: GitReviewFileDiffInput): Promise<GitReviewFileDiff> {
    return this.rpc.request<GitReviewFileDiff, GitReviewFileDiffInput>(
      GIT_GET_REVIEW_FILE_DIFF_METHOD,
      input
    )
  }

  getGitReviewFileContent(input: GitReviewFileContentInput): Promise<GitReviewFileContent> {
    return this.rpc.request<GitReviewFileContent, GitReviewFileContentInput>(
      GIT_GET_REVIEW_FILE_CONTENT_METHOD,
      input
    )
  }

  mutateGitReviewFile(input: GitReviewFileMutationInput): Promise<GitReviewFileMutation> {
    return this.rpc.request<GitReviewFileMutation, GitReviewFileMutationInput>(
      GIT_MUTATE_REVIEW_FILE_METHOD,
      input
    )
  }

  loadModelSettings(): Promise<StorageModelSettingsRecord | null> {
    return this.rpc.request<StorageModelSettingsRecord | null>(STORAGE_LOAD_MODEL_SETTINGS_METHOD)
  }

  loadProviderProfileUiDescriptors(): Promise<ProviderProfileUiDescriptor[]> {
    return this.rpc.request<ProviderProfileUiDescriptor[]>(
      STORAGE_LOAD_PROVIDER_PROFILE_UI_DESCRIPTORS_METHOD
    )
  }

  saveModelSettings(
    settings: StorageModelSettingsUpdateRecord
  ): Promise<StorageModelSettingsRecord> {
    return this.rpc.request<StorageModelSettingsRecord, StorageModelSettingsUpdateRecord>(
      STORAGE_SAVE_MODEL_SETTINGS_METHOD,
      settings
    )
  }

  loadAgentPromptPreferences(): Promise<StorageAgentPromptPreferencesRecord> {
    return this.rpc.request<StorageAgentPromptPreferencesRecord>(
      STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD
    )
  }

  saveAgentPromptPreferences(
    preferences: StorageAgentPromptPreferencesRecord
  ): Promise<StorageAgentPromptPreferencesRecord> {
    return this.rpc.request<
      StorageAgentPromptPreferencesRecord,
      StorageAgentPromptPreferencesRecord
    >(STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD, preferences)
  }

  loadProjects(): Promise<StorageProjectRecord[]> {
    return this.rpc.request<StorageProjectRecord[]>(STORAGE_LOAD_PROJECTS_METHOD)
  }

  saveProject(project: StorageProjectRecord): Promise<StorageProjectRecord> {
    return this.rpc.request<StorageProjectRecord, StorageProjectRecord>(
      STORAGE_SAVE_PROJECT_METHOD,
      project
    )
  }

  deleteProject(projectId: string): Promise<void> {
    return this.rpc.request<void, { projectId: string }>(STORAGE_DELETE_PROJECT_METHOD, {
      projectId
    })
  }

  loadConversations(): Promise<StorageChatConversationRecord[]> {
    return this.rpc.request<StorageChatConversationRecord[]>(STORAGE_LOAD_CONVERSATIONS_METHOD)
  }

  loadConversationMetas(): Promise<StorageChatConversationMetaRecord[]> {
    return this.rpc.request<StorageChatConversationMetaRecord[]>(
      STORAGE_LOAD_CONVERSATION_METAS_METHOD
    )
  }

  loadConversation(conversationId: string): Promise<StorageChatConversationRecord | null> {
    return this.rpc.request<StorageChatConversationRecord | null, { conversationId: string }>(
      STORAGE_LOAD_CONVERSATION_METHOD,
      { conversationId }
    )
  }

  forkConversation(input: StorageForkConversationRequest): Promise<StorageChatConversationRecord> {
    return this.rpc.request<StorageChatConversationRecord, StorageForkConversationRequest>(
      STORAGE_FORK_CONVERSATION_METHOD,
      input
    )
  }

  saveConversationMeta(
    conversation: StorageChatConversationMetaRecord
  ): Promise<StorageChatConversationMetaRecord> {
    return this.rpc.request<StorageChatConversationMetaRecord, StorageChatConversationMetaRecord>(
      STORAGE_SAVE_CONVERSATION_META_METHOD,
      conversation
    )
  }

  deleteConversation(conversationId: string): Promise<void> {
    return this.rpc.request<void, { conversationId: string }>(STORAGE_DELETE_CONVERSATION_METHOD, {
      conversationId
    })
  }

  deleteChatMessages(input: StorageDeleteChatMessagesRequest): Promise<void> {
    return this.rpc.request<void, StorageDeleteChatMessagesRequest>(
      STORAGE_DELETE_CHAT_MESSAGES_METHOD,
      input
    )
  }

  upsertChatMessages(input: {
    conversationId: string
    messages: StorageChatMessageRecord[]
    positionOffset: number
  }): Promise<StorageChatMessageRecord[]> {
    return this.rpc.request<StorageChatMessageRecord[], typeof input>(
      STORAGE_UPSERT_CHAT_MESSAGES_METHOD,
      input
    )
  }

  saveChatMessageState(input: {
    conversationId: string
    message: StorageChatMessageStateRecord
  }): Promise<void> {
    return this.rpc.request<void, typeof input>(STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD, input)
  }

  saveChatMessageUiState(input: {
    conversationId: string
    message: StorageChatMessageUiStateRecord
  }): Promise<void> {
    return this.rpc.request<void, typeof input>(STORAGE_SAVE_CHAT_MESSAGE_UI_STATE_METHOD, input)
  }

  loadComposerDrafts(): Promise<StorageComposerDraftRecord[]> {
    return this.rpc.request<StorageComposerDraftRecord[]>(STORAGE_LOAD_COMPOSER_DRAFTS_METHOD)
  }

  saveComposerDraft(draft: StorageComposerDraftRecord): Promise<StorageComposerDraftRecord> {
    return this.rpc.request<StorageComposerDraftRecord, { draft: StorageComposerDraftRecord }>(
      STORAGE_SAVE_COMPOSER_DRAFT_METHOD,
      { draft }
    )
  }

  loadUiPreferences(): Promise<StorageUiPreferencesRecord> {
    return this.rpc.request<StorageUiPreferencesRecord>(STORAGE_LOAD_UI_PREFERENCES_METHOD)
  }

  saveUiPreferences(preferences: StorageUiPreferencesRecord): Promise<StorageUiPreferencesRecord> {
    return this.rpc.request<StorageUiPreferencesRecord, StorageUiPreferencesRecord>(
      STORAGE_SAVE_UI_PREFERENCES_METHOD,
      preferences
    )
  }

  loadAttachmentImage(input: {
    attachmentId: string
  }): Promise<StorageAttachmentImageRecord | null> {
    return this.rpc.request<StorageAttachmentImageRecord | null, typeof input>(
      STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD,
      input
    )
  }

  loadInputAttachments(
    input: StorageLoadInputAttachmentsRequest
  ): Promise<StorageInputAttachment[]> {
    return this.rpc.request<StorageInputAttachment[], StorageLoadInputAttachmentsRequest>(
      STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD,
      input
    )
  }
}
