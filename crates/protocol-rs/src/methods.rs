pub const CORE_PING_METHOD: &str = "core.ping";
pub const CORE_SHUTDOWN_METHOD: &str = "core.shutdown";
pub const AUTOMATION_LIST_METHOD: &str = "automation.list";
pub const AUTOMATION_GET_METHOD: &str = "automation.get";
pub const AUTOMATION_CREATE_METHOD: &str = "automation.create";
pub const AUTOMATION_UPDATE_METHOD: &str = "automation.update";
pub const AUTOMATION_SET_ENABLED_METHOD: &str = "automation.setEnabled";
pub const AUTOMATION_RUN_NOW_METHOD: &str = "automation.runNow";
pub const AUTOMATION_DELETE_METHOD: &str = "automation.delete";
pub const AUTOMATION_RUNS_LIST_METHOD: &str = "automation.runs.list";
pub const AUTOMATION_ATTENTION_SUMMARY_METHOD: &str = "automation.attention.summary";
pub const AUTOMATION_ATTENTION_ACKNOWLEDGE_METHOD: &str = "automation.attention.acknowledge";
pub const AUTOMATION_EVENT_NOTIFICATION_METHOD: &str = "automation.event";
pub const AUTOMATION_RESYNC_NOTIFICATION_METHOD: &str = "automation.resync";
pub const NOTIFICATION_BATCHES_CLAIM_METHOD: &str = "notifications.claim";
pub const NOTIFICATION_BATCH_VALIDATE_METHOD: &str = "notifications.validate";
pub const NOTIFICATION_BATCH_ACKNOWLEDGE_METHOD: &str = "notifications.acknowledge";
pub const NOTIFICATION_BATCH_RELEASE_METHOD: &str = "notifications.release";
pub const NOTIFICATION_BATCH_SUPPRESS_METHOD: &str = "notifications.suppress";
pub const NOTIFICATION_LIST_METHOD: &str = "notifications.list";
pub const NOTIFICATION_SUMMARY_METHOD: &str = "notifications.summary";
pub const NOTIFICATION_MARK_SEEN_METHOD: &str = "notifications.markSeen";
pub const NOTIFICATION_SETTINGS_GET_METHOD: &str = "notifications.settings.get";
pub const NOTIFICATION_SETTINGS_UPDATE_METHOD: &str = "notifications.settings.update";
pub const NOTIFICATION_EVENT_NOTIFICATION_METHOD: &str = "notification.event";
pub const NOTIFICATION_RESYNC_NOTIFICATION_METHOD: &str = "notification.resync";
pub const MCP_SERVER_LIST_METHOD: &str = "mcp.server.list";
pub const MCP_SERVER_GET_METHOD: &str = "mcp.server.get";
pub const MCP_SERVER_ADD_METHOD: &str = "mcp.server.add";
pub const MCP_SERVER_UPDATE_METHOD: &str = "mcp.server.update";
pub const MCP_SERVER_DELETE_METHOD: &str = "mcp.server.delete";
pub const MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD: &str = "mcp.server.authorizeLaunch.prepare";
pub const MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD: &str = "mcp.server.authorizeLaunch.commit";
pub const MCP_SERVER_ENABLE_METHOD: &str = "mcp.server.enable";
pub const MCP_SERVER_DISABLE_METHOD: &str = "mcp.server.disable";
pub const MCP_SERVER_START_METHOD: &str = "mcp.server.start";
pub const MCP_SERVER_STOP_METHOD: &str = "mcp.server.stop";
pub const MCP_SERVER_RESTART_METHOD: &str = "mcp.server.restart";
pub const MCP_SERVER_STATUS_METHOD: &str = "mcp.server.status";
pub const MCP_CATALOG_TOOLS_METHOD: &str = "mcp.catalog.tools";
pub const MCP_CATALOG_REFRESH_METHOD: &str = "mcp.catalog.refresh";
pub const MCP_CHANGED_NOTIFICATION_METHOD: &str = "mcp.changed";
pub const MCP_BUILTIN_CAPABILITY_LIST_METHOD: &str = "mcp.builtinCapability.list";
pub const MCP_BUILTIN_CAPABILITY_SET_ALLOWED_METHOD: &str = "mcp.builtinCapability.setAllowed";
pub const OFFICE_GET_STATUS_METHOD: &str = "office.getStatus";
pub const IMAGE_GENERATION_GET_CONFIGURATION_METHOD: &str = "imageGeneration.getConfiguration";
pub const IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD: &str =
    "imageGeneration.updateConfiguration";
pub const IMAGE_GENERATION_SET_ENABLED_METHOD: &str = "imageGeneration.setEnabled";
pub const IMAGE_GENERATION_GET_STATUS_METHOD: &str = "imageGeneration.getStatus";
pub const IMAGE_GENERATION_READ_ARTIFACT_METHOD: &str = "imageGeneration.readArtifact";
pub const AGENT_CANCEL_RUN_METHOD: &str = "agent.cancelRun";
pub const AGENT_STEER_RUN_METHOD: &str = "agent.steerRun";
pub const AGENT_START_CONVERSATION_TURN_METHOD: &str = "agent.startConversationTurn";
pub const AGENT_REWRITE_CONVERSATION_TURN_METHOD: &str = "agent.rewriteConversationTurn";
pub const AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD: &str = "agent.getContextWindowSnapshot";
pub const AGENT_PREFLIGHT_PROVIDER_TRANSITION_METHOD: &str = "agent.preflightProviderTransition";
pub const AGENT_START_PROVIDER_TRANSITION_METHOD: &str = "agent.startProviderTransition";
pub const AGENT_GET_PROVIDER_TRANSITION_STATUS_METHOD: &str = "agent.getProviderTransitionStatus";
pub const AGENT_PROVIDER_TRANSITION_NOTIFICATION_METHOD: &str = "agent.providerTransition";
pub const AGENT_START_MANUAL_CONTEXT_COMPACTION_METHOD: &str = "agent.startManualContextCompaction";
pub const AGENT_GET_MANUAL_CONTEXT_COMPACTION_STATUS_METHOD: &str =
    "agent.getManualContextCompactionStatus";
pub const AGENT_CANCEL_MANUAL_CONTEXT_COMPACTION_METHOD: &str =
    "agent.cancelManualContextCompaction";
pub const AGENT_MANUAL_CONTEXT_COMPACTION_NOTIFICATION_METHOD: &str =
    "agent.manualContextCompaction";
pub const AGENT_COMMAND_SESSIONS_LIST_METHOD: &str = "agent.commandSessions.list";
pub const AGENT_COMMAND_SESSIONS_GET_METHOD: &str = "agent.commandSessions.get";
pub const AGENT_LIST_PENDING_ACTIONS_METHOD: &str = "agent.listPendingActions";
pub const AGENT_APPROVE_ACTION_METHOD: &str = "agent.approveAction";
pub const AGENT_REJECT_ACTION_METHOD: &str = "agent.rejectAction";
pub const AGENT_CANCEL_ACTION_METHOD: &str = "agent.cancelAction";
pub const AGENT_GET_USAGE_SUMMARY_METHOD: &str = "agent.getUsageSummary";
pub const AGENT_CLEAR_USAGE_RECORDS_METHOD: &str = "agent.clearUsageRecords";
pub const AGENT_READ_FILE_CHANGE_METHOD: &str = "agent.readFileChange";
pub const AGENT_GET_FILE_CHANGE_DIFF_METHOD: &str = "agent.getFileChangeDiff";
pub const AGENT_GET_FILE_CHANGE_HISTORY_DIFF_METHOD: &str = "agent.getFileChangeHistoryDiff";
pub const AGENT_EVENT_NOTIFICATION_METHOD: &str = "agent.event";
pub const AGENT_COLLABORATION_GET_TREE_METHOD: &str = "agent.collaboration.getTree";
pub const AGENT_COLLABORATION_GET_AGENT_METHOD: &str = "agent.collaboration.getAgent";
pub const AGENT_COLLABORATION_LOCATE_CONVERSATION_METHOD: &str =
    "agent.collaboration.locateConversation";
pub const AGENT_COLLABORATION_LOAD_OBSERVER_CONVERSATION_METHOD: &str =
    "agent.collaboration.loadObserverConversation";
pub const AGENT_COLLABORATION_LIST_EVENTS_METHOD: &str = "agent.collaboration.listEvents";
pub const AGENT_COLLABORATION_TEMPLATES_LIST_METHOD: &str = "agent.collaboration.templates.list";
pub const AGENT_COLLABORATION_TEMPLATES_CREATE_METHOD: &str =
    "agent.collaboration.templates.create";
pub const AGENT_COLLABORATION_TEMPLATES_UPDATE_METHOD: &str =
    "agent.collaboration.templates.update";
pub const AGENT_COLLABORATION_TEMPLATES_SET_ENABLED_METHOD: &str =
    "agent.collaboration.templates.setEnabled";
pub const AGENT_COLLABORATION_TEMPLATES_SET_PROJECT_ASSIGNMENT_METHOD: &str =
    "agent.collaboration.templates.setProjectAssignment";
pub const AGENT_COLLABORATION_TEMPLATES_DELETE_METHOD: &str =
    "agent.collaboration.templates.delete";
pub const AGENT_COLLABORATION_APPROVALS_LIST_METHOD: &str = "agent.collaboration.approvals.list";
pub const AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD: &str =
    "agent.collaboration.approvals.decide";
pub const AGENT_COLLABORATION_EVENT_NOTIFICATION_METHOD: &str = "agent.collaboration.event";
pub const AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD: &str =
    "agent.collaboration.observerEvent";
pub const AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD: &str = "agent.collaboration.resync";
pub const SEARCH_SEARCH_CHATS_METHOD: &str = "search.searchChats";
pub const SKILLS_LIST_METHOD: &str = "skills.list";
pub const SKILLS_INSTALL_LOCAL_METHOD: &str = "skills.installLocal";
pub const SKILLS_UPDATE_LOCAL_METHOD: &str = "skills.updateLocal";
pub const SKILLS_UNINSTALL_METHOD: &str = "skills.uninstall";
pub const SKILLS_INSPECT_INSTALLATION_METHOD: &str = "skills.inspectInstallation";
pub const SKILLS_COMMIT_INSTALLATION_METHOD: &str = "skills.commitInstallation";
pub const SKILLS_CANCEL_PREPARATION_METHOD: &str = "skills.cancelPreparation";
pub const SKILLS_LIST_MANAGEMENT_METHOD: &str = "skills.listManagement";
pub const SKILLS_SET_ENABLED_METHOD: &str = "skills.setEnabled";
pub const SKILLS_CHANGED_NOTIFICATION_METHOD: &str = "skills.changed";
pub const SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD: &str = "skills.resolveInstallationSource";
pub const SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD: &str = "skills.cancelSourceResolution";
pub const GIT_INSPECT_REPOSITORY_METHOD: &str = "git.inspectRepository";
pub const GIT_GET_REVIEW_REPOSITORY_CONTEXT_METHOD: &str = "git.getReviewRepositoryContext";
pub const GIT_LIST_REVIEW_COMMITS_METHOD: &str = "git.listReviewCommits";
pub const GIT_GET_REVIEW_SUMMARY_METHOD: &str = "git.getReviewSummary";
pub const GIT_GET_TURN_DIFF_SUMMARIES_METHOD: &str = "git.getTurnDiffSummaries";
pub const GIT_GET_REVIEW_FILE_DIFF_METHOD: &str = "git.getReviewFileDiff";
pub const GIT_GET_REVIEW_FILE_CONTENT_METHOD: &str = "git.getReviewFileContent";
pub const GIT_MUTATE_REVIEW_FILE_METHOD: &str = "git.mutateReviewFile";
pub const STORAGE_LOAD_MODEL_SETTINGS_METHOD: &str = "storage.loadModelSettings";
pub const STORAGE_LOAD_PROVIDER_PROFILE_UI_DESCRIPTORS_METHOD: &str =
    "storage.loadProviderProfileUiDescriptors";
pub const STORAGE_LOAD_PROVIDER_VENDOR_DESCRIPTORS_METHOD: &str =
    "storage.loadProviderVendorDescriptors";
pub const STORAGE_RESOLVE_PROVIDER_VENDOR_MODEL_POLICY_METHOD: &str =
    "storage.resolveProviderVendorModelPolicy";
pub const STORAGE_SAVE_MODEL_SETTINGS_METHOD: &str = "storage.saveModelSettings";
pub const STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD: &str = "storage.loadAgentPromptPreferences";
pub const STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD: &str = "storage.saveAgentPromptPreferences";
pub const STORAGE_LOAD_PROJECTS_METHOD: &str = "storage.loadProjects";
pub const STORAGE_SAVE_PROJECT_METHOD: &str = "storage.saveProject";
pub const STORAGE_DELETE_PROJECT_METHOD: &str = "storage.deleteProject";
pub const STORAGE_LOAD_CONVERSATIONS_METHOD: &str = "storage.loadConversations";
pub const STORAGE_LOAD_CONVERSATION_METAS_METHOD: &str = "storage.loadConversationMetas";
pub const STORAGE_LOAD_CONVERSATION_METHOD: &str = "storage.loadConversation";
pub const STORAGE_SAVE_CONVERSATION_META_METHOD: &str = "storage.saveConversationMeta";
pub const STORAGE_DELETE_CONVERSATION_METHOD: &str = "storage.deleteConversation";
pub const STORAGE_DELETE_CHAT_MESSAGES_METHOD: &str = "storage.deleteChatMessages";
pub const STORAGE_FORK_CONVERSATION_METHOD: &str = "storage.forkConversation";
pub const STORAGE_UPSERT_CHAT_MESSAGES_METHOD: &str = "storage.upsertChatMessages";
pub const STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD: &str = "storage.saveChatMessageState";
pub const STORAGE_SAVE_CHAT_MESSAGE_UI_STATE_METHOD: &str = "storage.saveChatMessageUiState";
pub const STORAGE_LOAD_COMPOSER_DRAFTS_METHOD: &str = "storage.loadComposerDrafts";
pub const STORAGE_SAVE_COMPOSER_DRAFT_METHOD: &str = "storage.saveComposerDraft";
pub const STORAGE_SAVE_COMPOSER_DRAFT_MESSAGE_METHOD: &str = "storage.saveComposerDraftMessage";
pub const STORAGE_LOAD_UI_PREFERENCES_METHOD: &str = "storage.loadUiPreferences";
pub const STORAGE_SAVE_UI_PREFERENCES_METHOD: &str = "storage.saveUiPreferences";
pub const STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD: &str = "storage.loadAttachmentImage";
pub const STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD: &str = "storage.loadInputAttachments";
pub const STORAGE_LOAD_BROWSER_DOWNLOAD_SETTINGS_METHOD: &str =
    "storage.loadBrowserDownloadSettings";
pub const STORAGE_SAVE_BROWSER_DOWNLOAD_SETTINGS_METHOD: &str =
    "storage.saveBrowserDownloadSettings";
pub const STORAGE_REGISTER_BROWSER_DOWNLOAD_METHOD: &str = "storage.registerBrowserDownload";
pub const STORAGE_LIST_BROWSER_DOWNLOADS_METHOD: &str = "storage.listBrowserDownloads";
pub const STORAGE_LOAD_BROWSER_DOWNLOAD_METHOD: &str = "storage.loadBrowserDownload";
pub const STORAGE_CLEAR_BROWSER_DOWNLOAD_HISTORY_METHOD: &str =
    "storage.clearBrowserDownloadHistory";
pub const STORAGE_LOAD_BROWSER_PREFERENCES_METHOD: &str = "storage.loadBrowserPreferences";
pub const STORAGE_SAVE_BROWSER_PREFERENCES_METHOD: &str = "storage.saveBrowserPreferences";
pub const STORAGE_REGISTER_BROWSER_HISTORY_METHOD: &str = "storage.registerBrowserHistory";
pub const STORAGE_UPDATE_BROWSER_HISTORY_METHOD: &str = "storage.updateBrowserHistoryMetadata";
pub const STORAGE_LIST_BROWSER_HISTORY_METHOD: &str = "storage.listBrowserHistory";
pub const STORAGE_DELETE_BROWSER_HISTORY_METHOD: &str = "storage.deleteBrowserHistory";
pub const STORAGE_SUMMARIZE_BROWSER_OWNED_DATA_METHOD: &str = "storage.summarizeBrowserOwnedData";
pub const STORAGE_CLEAR_BROWSER_OWNED_DATA_METHOD: &str = "storage.clearBrowserOwnedData";
