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
  AgentRejectActionRequest,
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
  StorageComposerDraftRecord,
  StorageModelSettingsRecord,
  StorageProjectRecord,
  StorageUiPreferencesRecord
} from '@mycopilot/protocol'
import {
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
  SKILLS_UNINSTALL_METHOD,
  SKILLS_UPDATE_LOCAL_METHOD
} from '@mycopilot/protocol'

import { CoreJsonRpcClient } from './jsonRpcClient'

const CORE_PING_METHOD = 'core.ping'
const CORE_SHUTDOWN_METHOD = 'core.shutdown'
const AGENT_CANCEL_RUN_METHOD = 'agent.cancelRun'
const AGENT_START_CONVERSATION_TURN_METHOD = 'agent.startConversationTurn'
const AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD = 'agent.getContextWindowSnapshot'
const AGENT_GET_CONTEXT_COMPACTION_AUDIT_METHOD = 'agent.getContextCompactionAudit'
const AGENT_LIST_PENDING_ACTIONS_METHOD = 'agent.listPendingActions'
const AGENT_APPROVE_ACTION_METHOD = 'agent.approveAction'
const AGENT_REJECT_ACTION_METHOD = 'agent.rejectAction'
const AGENT_CANCEL_ACTION_METHOD = 'agent.cancelAction'
const AGENT_GET_USAGE_SUMMARY_METHOD = 'agent.getUsageSummary'
const AGENT_CLEAR_USAGE_RECORDS_METHOD = 'agent.clearUsageRecords'
const AGENT_READ_FILE_DRAFT_METHOD = 'agent.readFileDraft'
const AGENT_GET_FILE_WRITE_DIFF_METHOD = 'agent.getFileWriteDiff'
const AGENT_EVENT_NOTIFICATION_METHOD = 'agent.event'
const SEARCH_SEARCH_CHATS_METHOD = 'search.searchChats'
const SKILLS_LIST_METHOD = 'skills.list'
const GIT_INSPECT_REPOSITORY_METHOD = 'git.inspectRepository'
const GIT_GET_REVIEW_SUMMARY_METHOD = 'git.getReviewSummary'
const GIT_GET_REVIEW_FILE_DIFF_METHOD = 'git.getReviewFileDiff'
const GIT_GET_REVIEW_FILE_CONTENT_METHOD = 'git.getReviewFileContent'
const GIT_MUTATE_REVIEW_FILE_METHOD = 'git.mutateReviewFile'
const STORAGE_LOAD_MODEL_SETTINGS_METHOD = 'storage.loadModelSettings'
const STORAGE_SAVE_MODEL_SETTINGS_METHOD = 'storage.saveModelSettings'
const STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD = 'storage.loadAgentPromptPreferences'
const STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD = 'storage.saveAgentPromptPreferences'
const STORAGE_LOAD_PROJECTS_METHOD = 'storage.loadProjects'
const STORAGE_SAVE_PROJECT_METHOD = 'storage.saveProject'
const STORAGE_DELETE_PROJECT_METHOD = 'storage.deleteProject'
const STORAGE_LOAD_CONVERSATIONS_METHOD = 'storage.loadConversations'
const STORAGE_SAVE_CONVERSATION_META_METHOD = 'storage.saveConversationMeta'
const STORAGE_DELETE_CONVERSATION_METHOD = 'storage.deleteConversation'
const STORAGE_DELETE_CHAT_MESSAGES_METHOD = 'storage.deleteChatMessages'
const STORAGE_FORK_CONVERSATION_METHOD = 'storage.forkConversation'
const STORAGE_UPSERT_CHAT_MESSAGES_METHOD = 'storage.upsertChatMessages'
const STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD = 'storage.saveChatMessageState'
const STORAGE_LOAD_COMPOSER_DRAFTS_METHOD = 'storage.loadComposerDrafts'
const STORAGE_SAVE_COMPOSER_DRAFT_METHOD = 'storage.saveComposerDraft'
const STORAGE_LOAD_UI_PREFERENCES_METHOD = 'storage.loadUiPreferences'
const STORAGE_SAVE_UI_PREFERENCES_METHOD = 'storage.saveUiPreferences'
const STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD = 'storage.loadAttachmentImage'
const STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD = 'storage.loadInputAttachments'

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

export class CoreServer {
  private readonly rpc = new CoreJsonRpcClient()

  start(): void {
    this.rpc.start()
  }

  stop(): void {
    this.rpc.stop()
  }

  async shutdown(): Promise<void> {
    if (!this.rpc.isRunning()) return

    let timeoutId: ReturnType<typeof setTimeout> | null = null
    const timeout = new Promise<void>((resolve) => {
      timeoutId = setTimeout(resolve, 2500)
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

  startConversationTurn(input: AgentConversationTurnInput): Promise<AgentConversationTurnOutput> {
    return this.rpc.request<AgentConversationTurnOutput, AgentConversationTurnInput>(
      AGENT_START_CONVERSATION_TURN_METHOD,
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

  getContextCompactionAudit(
    input: AgentContextCompactionAuditInput
  ): Promise<AgentContextCompactionAuditOutput> {
    return this.rpc.request<AgentContextCompactionAuditOutput, AgentContextCompactionAuditInput>(
      AGENT_GET_CONTEXT_COMPACTION_AUDIT_METHOD,
      input
    )
  }

  cancelRun(input: AgentCancelRunRequest): Promise<AgentCancelRunResponse> {
    return this.rpc.request<AgentCancelRunResponse, AgentCancelRunRequest>(
      AGENT_CANCEL_RUN_METHOD,
      input
    )
  }

  listPendingActions(): Promise<PendingAgentActionSnapshot[]> {
    return this.rpc.request<PendingAgentActionSnapshot[]>(AGENT_LIST_PENDING_ACTIONS_METHOD)
  }

  approveAction(input: AgentActionIdRequest): Promise<AgentActionExecutionOutput> {
    return this.rpc.request<AgentActionExecutionOutput, AgentActionIdRequest>(
      AGENT_APPROVE_ACTION_METHOD,
      input
    )
  }

  rejectAction(input: AgentRejectActionRequest): Promise<AgentActionExecutionOutput> {
    return this.rpc.request<AgentActionExecutionOutput, AgentRejectActionRequest>(
      AGENT_REJECT_ACTION_METHOD,
      input
    )
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
    return this.rpc.onNotification(AGENT_EVENT_NOTIFICATION_METHOD, (params) =>
      handler(params as AgentEvent)
    )
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

  saveModelSettings(settings: StorageModelSettingsRecord): Promise<void> {
    return this.rpc.request<void, StorageModelSettingsRecord>(
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
