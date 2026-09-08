import type {
  BrowserDownloadListInput,
  BrowserDownloadRecord,
  BrowserDownloadRegistrationInput,
  BrowserDownloadSettingsRecord,
  BrowserHistoryDeleteInput,
  BrowserHistoryEntry,
  BrowserHistoryListInput,
  BrowserHistoryMetadataUpdateInput,
  BrowserHistoryRegistrationInput,
  BrowserOwnedDataClearInput,
  BrowserOwnedDataClearOutput,
  BrowserOwnedDataRangeInput,
  BrowserOwnedDataSummary,
  BrowserPreferencesSaveInput,
  BrowserPreferencesView,
  ChatSearchInput,
  ChatSearchResult,
  GitRepositoryInspectInput,
  GitRepositoryInspection,
  GitReviewCommitList,
  GitReviewCommitListInput,
  GitReviewFileContent,
  GitReviewFileContentInput,
  GitReviewFileDiff,
  GitReviewFileDiffInput,
  GitReviewFileMutation,
  GitReviewFileMutationInput,
  GitReviewSummary,
  GitReviewSummaryInput,
  GitReviewRepositoryContext,
  GitReviewRepositoryContextInput,
  GitTurnDiffSummaries,
  GitTurnDiffSummariesInput,
  ProviderProfileUiDescriptor,
  ProviderVendorDescriptor,
  ProviderVendorModelPolicyDescriptor,
  ProviderVendorModelPolicyInput,
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
  StorageChatMessageRecord,
  StorageChatMessageStateRecord,
  StorageChatMessageUiStateRecord,
  StorageComposerDraftMessageUpdate,
  StorageComposerDraftRecord,
  StorageDeleteChatMessagesRequest,
  StorageForkConversationRequest,
  StorageInputAttachment,
  StorageLoadInputAttachmentsRequest,
  StorageModelSettingsRecord,
  StorageModelSettingsUpdateRecord,
  StorageProjectRecord,
  StorageUiPreferencesRecord
} from '@mycopilot/protocol'
import {
  SKILL_INSPECTION_ERROR_CODE,
  SKILL_MANAGEMENT_ERROR_CODE,
  SKILL_SOURCE_RESOLUTION_ERROR_CODE,
  SKILLS_CANCEL_PREPARATION_METHOD,
  SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD,
  SKILLS_CHANGED_NOTIFICATION_METHOD,
  SKILLS_COMMIT_INSTALLATION_METHOD,
  SKILLS_INSPECT_INSTALLATION_METHOD,
  SKILLS_LIST_MANAGEMENT_METHOD,
  SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD,
  SKILLS_SET_ENABLED_METHOD,
  parseBrowserDownloadRecord,
  parseBrowserDownloadRegistrationInput,
  parseBrowserDownloadSettingsRecord,
  parseBrowserHistoryDeleteInput,
  parseBrowserHistoryEntry,
  parseBrowserHistoryListInput,
  parseBrowserHistoryMetadataUpdateInput,
  parseBrowserHistoryRegistrationInput,
  parseBrowserOwnedDataClearInput,
  parseBrowserOwnedDataClearOutput,
  parseBrowserOwnedDataRangeInput,
  parseBrowserOwnedDataSummary,
  parseBrowserPreferencesSaveInput,
  parseBrowserPreferencesView,
  parseProviderProfileUiDescriptors,
  parseProviderVendorDescriptors,
  parseProviderVendorModelPolicyDescriptor,
  parseSkillAcquisitionSource,
  parseSkillInstallationCommitOutput,
  parseSkillInstallationPreview,
  parseSkillInspectionErrorData,
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
  parseStorageModelSettingsRecord,
  parseStorageModelSettingsUpdateRecord,
  SKILLS_INSTALL_LOCAL_METHOD,
  SKILLS_UNINSTALL_METHOD,
  SKILLS_UPDATE_LOCAL_METHOD
} from '@mycopilot/protocol'

import { CoreJsonRpcClient } from './jsonRpcClient'

const SEARCH_SEARCH_CHATS_METHOD = 'search.searchChats'
const SKILLS_LIST_METHOD = 'skills.list'
const GIT_INSPECT_REPOSITORY_METHOD = 'git.inspectRepository'
const GIT_GET_REVIEW_REPOSITORY_CONTEXT_METHOD = 'git.getReviewRepositoryContext'
const GIT_LIST_REVIEW_COMMITS_METHOD = 'git.listReviewCommits'
const GIT_GET_REVIEW_SUMMARY_METHOD = 'git.getReviewSummary'
const GIT_GET_TURN_DIFF_SUMMARIES_METHOD = 'git.getTurnDiffSummaries'
const GIT_GET_REVIEW_FILE_DIFF_METHOD = 'git.getReviewFileDiff'
const GIT_GET_REVIEW_FILE_CONTENT_METHOD = 'git.getReviewFileContent'
const GIT_MUTATE_REVIEW_FILE_METHOD = 'git.mutateReviewFile'
const STORAGE_LOAD_MODEL_SETTINGS_METHOD = 'storage.loadModelSettings'
const STORAGE_LOAD_PROVIDER_PROFILE_UI_DESCRIPTORS_METHOD =
  'storage.loadProviderProfileUiDescriptors'
const STORAGE_LOAD_PROVIDER_VENDOR_DESCRIPTORS_METHOD = 'storage.loadProviderVendorDescriptors'
const STORAGE_RESOLVE_PROVIDER_VENDOR_MODEL_POLICY_METHOD =
  'storage.resolveProviderVendorModelPolicy'
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
const STORAGE_SAVE_COMPOSER_DRAFT_MESSAGE_METHOD = 'storage.saveComposerDraftMessage'
const STORAGE_LOAD_UI_PREFERENCES_METHOD = 'storage.loadUiPreferences'
const STORAGE_SAVE_UI_PREFERENCES_METHOD = 'storage.saveUiPreferences'
const STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD = 'storage.loadAttachmentImage'
const STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD = 'storage.loadInputAttachments'
const STORAGE_LOAD_BROWSER_DOWNLOAD_SETTINGS_METHOD = 'storage.loadBrowserDownloadSettings'
const STORAGE_SAVE_BROWSER_DOWNLOAD_SETTINGS_METHOD = 'storage.saveBrowserDownloadSettings'
const STORAGE_REGISTER_BROWSER_DOWNLOAD_METHOD = 'storage.registerBrowserDownload'
const STORAGE_LIST_BROWSER_DOWNLOADS_METHOD = 'storage.listBrowserDownloads'
const STORAGE_LOAD_BROWSER_DOWNLOAD_METHOD = 'storage.loadBrowserDownload'
const STORAGE_CLEAR_BROWSER_DOWNLOAD_HISTORY_METHOD = 'storage.clearBrowserDownloadHistory'
const STORAGE_LOAD_BROWSER_PREFERENCES_METHOD = 'storage.loadBrowserPreferences'
const STORAGE_SAVE_BROWSER_PREFERENCES_METHOD = 'storage.saveBrowserPreferences'
const STORAGE_REGISTER_BROWSER_HISTORY_METHOD = 'storage.registerBrowserHistory'
const STORAGE_UPDATE_BROWSER_HISTORY_METHOD = 'storage.updateBrowserHistoryMetadata'
const STORAGE_LIST_BROWSER_HISTORY_METHOD = 'storage.listBrowserHistory'
const STORAGE_DELETE_BROWSER_HISTORY_METHOD = 'storage.deleteBrowserHistory'
const STORAGE_SUMMARIZE_BROWSER_OWNED_DATA_METHOD = 'storage.summarizeBrowserOwnedData'
const STORAGE_CLEAR_BROWSER_OWNED_DATA_METHOD = 'storage.clearBrowserOwnedData'

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

export type HostConfigurationDomain = 'modelSettings' | 'imageGeneration' | 'builtinCapabilities'

/** Request and invalidation facade. CoreServer owns the RPC process lifecycle. */
export class CoreServerStorageApi {
  private readonly configurationHandlers = new Map<HostConfigurationDomain, Set<() => void>>()

  protected constructor(protected readonly rpc: CoreJsonRpcClient) {}

  /** An invalidation is not a successful-write receipt. Consumers always requery Core. */
  onConfigurationInvalidated(domain: HostConfigurationDomain, handler: () => void): () => void {
    const handlers = this.configurationHandlers.get(domain) ?? new Set<() => void>()
    handlers.add(handler)
    this.configurationHandlers.set(domain, handlers)
    return () => {
      handlers.delete(handler)
      if (!handlers.size && this.configurationHandlers.get(domain) === handlers) {
        this.configurationHandlers.delete(domain)
      }
    }
  }

  protected invalidateConfiguration(domain: HostConfigurationDomain): void {
    for (const handler of [...(this.configurationHandlers.get(domain) ?? [])]) {
      try {
        handler()
      } catch {
        // Observers cannot change the outcome of a committed or uncertain mutation.
        console.warn('Host configuration invalidation listener failed')
      }
    }
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
      .finally(() => {
        if (input.skillId === 'bundled:application:image-generation') {
          this.invalidateConfiguration('imageGeneration')
        }
      })
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

  getGitReviewRepositoryContext(
    input: GitReviewRepositoryContextInput
  ): Promise<GitReviewRepositoryContext> {
    return this.rpc.request<GitReviewRepositoryContext, GitReviewRepositoryContextInput>(
      GIT_GET_REVIEW_REPOSITORY_CONTEXT_METHOD,
      input
    )
  }

  listGitReviewCommits(input: GitReviewCommitListInput): Promise<GitReviewCommitList> {
    return this.rpc.request<GitReviewCommitList, GitReviewCommitListInput>(
      GIT_LIST_REVIEW_COMMITS_METHOD,
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
    return this.rpc
      .request<unknown>(STORAGE_LOAD_MODEL_SETTINGS_METHOD)
      .then((value) => (value === null ? null : parseStorageModelSettingsRecord(value)))
  }

  loadProviderProfileUiDescriptors(): Promise<ProviderProfileUiDescriptor[]> {
    return this.rpc
      .request<unknown>(STORAGE_LOAD_PROVIDER_PROFILE_UI_DESCRIPTORS_METHOD)
      .then(parseProviderProfileUiDescriptors)
  }

  loadProviderVendorDescriptors(): Promise<ProviderVendorDescriptor[]> {
    return this.rpc
      .request<unknown>(STORAGE_LOAD_PROVIDER_VENDOR_DESCRIPTORS_METHOD)
      .then(parseProviderVendorDescriptors)
  }

  resolveProviderVendorModelPolicy(
    input: ProviderVendorModelPolicyInput
  ): Promise<ProviderVendorModelPolicyDescriptor> {
    return this.rpc
      .request<unknown, ProviderVendorModelPolicyInput>(
        STORAGE_RESOLVE_PROVIDER_VENDOR_MODEL_POLICY_METHOD,
        input
      )
      .then(parseProviderVendorModelPolicyDescriptor)
  }

  saveModelSettings(
    settings: StorageModelSettingsUpdateRecord
  ): Promise<StorageModelSettingsRecord> {
    return this.rpc
      .request<unknown, StorageModelSettingsUpdateRecord>(
        STORAGE_SAVE_MODEL_SETTINGS_METHOD,
        parseStorageModelSettingsUpdateRecord(settings)
      )
      .then(parseStorageModelSettingsRecord)
      .finally(() => this.invalidateConfiguration('modelSettings'))
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

  saveComposerDraftMessage(input: StorageComposerDraftMessageUpdate): Promise<boolean> {
    return this.rpc.request<boolean, StorageComposerDraftMessageUpdate>(
      STORAGE_SAVE_COMPOSER_DRAFT_MESSAGE_METHOD,
      input
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

  loadBrowserDownloadSettings(): Promise<BrowserDownloadSettingsRecord> {
    return this.rpc
      .request<unknown>(STORAGE_LOAD_BROWSER_DOWNLOAD_SETTINGS_METHOD)
      .then(parseBrowserDownloadSettingsRecord)
  }

  saveBrowserDownloadSettings(input: {
    schemaVersion: 2
    locationMode: 'system' | 'custom'
    customDirectory: string | null
    askWhereToSave: boolean
    expectedRevision: number
    updatedAt: number
  }): Promise<BrowserDownloadSettingsRecord> {
    return this.rpc
      .request<unknown, typeof input>(STORAGE_SAVE_BROWSER_DOWNLOAD_SETTINGS_METHOD, input)
      .then(parseBrowserDownloadSettingsRecord)
  }

  registerBrowserDownload(input: BrowserDownloadRegistrationInput): Promise<BrowserDownloadRecord> {
    const parsed = parseBrowserDownloadRegistrationInput(input)
    return this.rpc
      .request<unknown, BrowserDownloadRegistrationInput>(
        STORAGE_REGISTER_BROWSER_DOWNLOAD_METHOD,
        parsed
      )
      .then(parseBrowserDownloadRecord)
  }

  listBrowserDownloads(input: BrowserDownloadListInput): Promise<BrowserDownloadRecord[]> {
    return this.rpc
      .request<unknown, BrowserDownloadListInput>(STORAGE_LIST_BROWSER_DOWNLOADS_METHOD, input)
      .then((value) => {
        if (!Array.isArray(value)) throw new Error('Invalid Browser Download record list')
        return value.map((item) => parseBrowserDownloadRecord(item))
      })
  }

  loadBrowserDownload(downloadId: string): Promise<BrowserDownloadRecord | null> {
    return this.rpc
      .request<unknown, { downloadId: string }>(STORAGE_LOAD_BROWSER_DOWNLOAD_METHOD, {
        downloadId
      })
      .then((value) => (value === null ? null : parseBrowserDownloadRecord(value)))
  }

  clearBrowserDownloadHistory(): Promise<number> {
    return this.rpc.request<number>(STORAGE_CLEAR_BROWSER_DOWNLOAD_HISTORY_METHOD)
  }

  loadBrowserPreferences(): Promise<BrowserPreferencesView> {
    return this.rpc
      .request<unknown>(STORAGE_LOAD_BROWSER_PREFERENCES_METHOD)
      .then(parseBrowserPreferencesView)
  }

  saveBrowserPreferences(input: BrowserPreferencesSaveInput): Promise<BrowserPreferencesView> {
    const parsed = parseBrowserPreferencesSaveInput(input)
    return this.rpc
      .request<unknown, BrowserPreferencesSaveInput>(
        STORAGE_SAVE_BROWSER_PREFERENCES_METHOD,
        parsed
      )
      .then(parseBrowserPreferencesView)
  }

  registerBrowserHistory(input: BrowserHistoryRegistrationInput): Promise<BrowserHistoryEntry> {
    const parsed = parseBrowserHistoryRegistrationInput(input)
    return this.rpc
      .request<unknown, BrowserHistoryRegistrationInput>(
        STORAGE_REGISTER_BROWSER_HISTORY_METHOD,
        parsed
      )
      .then(parseBrowserHistoryEntry)
  }

  updateBrowserHistoryMetadata(input: BrowserHistoryMetadataUpdateInput): Promise<boolean> {
    return this.rpc.request<boolean, BrowserHistoryMetadataUpdateInput>(
      STORAGE_UPDATE_BROWSER_HISTORY_METHOD,
      parseBrowserHistoryMetadataUpdateInput(input)
    )
  }

  listBrowserHistory(input: BrowserHistoryListInput): Promise<BrowserHistoryEntry[]> {
    const parsed = parseBrowserHistoryListInput(input)
    return this.rpc
      .request<unknown, BrowserHistoryListInput>(STORAGE_LIST_BROWSER_HISTORY_METHOD, parsed)
      .then((value) => {
        if (!Array.isArray(value)) throw new Error('Invalid Browser history record list')
        return value.map((item) => parseBrowserHistoryEntry(item))
      })
  }

  deleteBrowserHistory(input: BrowserHistoryDeleteInput): Promise<number> {
    return this.rpc.request<number, BrowserHistoryDeleteInput>(
      STORAGE_DELETE_BROWSER_HISTORY_METHOD,
      parseBrowserHistoryDeleteInput(input)
    )
  }

  summarizeBrowserOwnedData(input: BrowserOwnedDataRangeInput): Promise<BrowserOwnedDataSummary> {
    return this.rpc
      .request<unknown, BrowserOwnedDataRangeInput>(
        STORAGE_SUMMARIZE_BROWSER_OWNED_DATA_METHOD,
        parseBrowserOwnedDataRangeInput(input)
      )
      .then(parseBrowserOwnedDataSummary)
  }

  clearBrowserOwnedData(input: BrowserOwnedDataClearInput): Promise<BrowserOwnedDataClearOutput> {
    return this.rpc
      .request<unknown, BrowserOwnedDataClearInput>(
        STORAGE_CLEAR_BROWSER_OWNED_DATA_METHOD,
        parseBrowserOwnedDataClearInput(input)
      )
      .then(parseBrowserOwnedDataClearOutput)
  }
}
