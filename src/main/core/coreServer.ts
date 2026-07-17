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
  StorageModelSettingsRecord,
  StorageProjectRecord,
  StorageUiPreferencesRecord
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
