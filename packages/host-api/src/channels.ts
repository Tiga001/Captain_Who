/**
 * The single source of truth for the context-isolated Electron Host API transport.
 * Keep these names out of main/preload implementations so a channel rename cannot drift.
 */
export const HOST_CHANNELS = {
  core: {
    ping: 'host:core.ping'
  },
  app: {
    getWindowState: 'host:app.getWindowState',
    openExternal: 'host:app.openExternal',
    setNativeThemeSource: 'host:app.setNativeThemeSource',
    windowStateChange: 'host:app.windowStateChange'
  },
  agent: {
    collaborationApprovalDecide: 'host:agent.collaboration.approvals.decide',
    collaborationApprovalList: 'host:agent.collaboration.approvals.list',
    collaborationEvent: 'host:agent.collaboration.event',
    collaborationObserverEvent: 'host:agent.collaboration.observerEvent',
    collaborationResync: 'host:agent.collaboration.resync',
    collaborationGetAgent: 'host:agent.collaboration.getAgent',
    collaborationGetTree: 'host:agent.collaboration.getTree',
    collaborationListEvents: 'host:agent.collaboration.listEvents',
    collaborationLoadObserverConversation: 'host:agent.collaboration.loadObserverConversation',
    collaborationLocateConversation: 'host:agent.collaboration.locateConversation',
    collaborationTemplateCreate: 'host:agent.collaboration.templates.create',
    collaborationTemplateDelete: 'host:agent.collaboration.templates.delete',
    collaborationTemplateList: 'host:agent.collaboration.templates.list',
    collaborationTemplateSetEnabled: 'host:agent.collaboration.templates.setEnabled',
    collaborationTemplateUpdate: 'host:agent.collaboration.templates.update',
    approveAction: 'host:agent.approveAction',
    cancelAction: 'host:agent.cancelAction',
    cancelRun: 'host:agent.cancelRun',
    clearUsageRecords: 'host:agent.clearUsageRecords',
    event: 'host:agent.event',
    getCommandSession: 'host:agent.getCommandSession',
    getContextWindowSnapshot: 'host:agent.getContextWindowSnapshot',
    getFileWriteDiff: 'host:agent.getFileWriteDiff',
    getProviderTransitionStatus: 'host:agent.getProviderTransitionStatus',
    getUsageSummary: 'host:agent.getUsageSummary',
    listCommandSessions: 'host:agent.listCommandSessions',
    listPendingActions: 'host:agent.listPendingActions',
    readFileDraft: 'host:agent.readFileDraft',
    rejectAction: 'host:agent.rejectAction',
    preflightProviderTransition: 'host:agent.preflightProviderTransition',
    providerTransition: 'host:agent.providerTransition',
    startProviderTransition: 'host:agent.startProviderTransition',
    startConversationTurn: 'host:agent.startConversationTurn',
    steerRun: 'host:agent.steerRun'
  },
  attachments: {
    selectInputAttachments: 'host:attachments.selectInputAttachments'
  },
  browser: {
    clearBrowsingData: 'host:browser.clearBrowsingData'
  },
  git: {
    getReviewFileContent: 'host:git.getReviewFileContent',
    getReviewFileDiff: 'host:git.getReviewFileDiff',
    getReviewSummary: 'host:git.getReviewSummary',
    getTurnDiffSummaries: 'host:git.getTurnDiffSummaries',
    inspectRepository: 'host:git.inspectRepository',
    mutateReviewFile: 'host:git.mutateReviewFile'
  },
  imageGeneration: {
    getConfiguration: 'host:imageGeneration.getConfiguration',
    getStatus: 'host:imageGeneration.getStatus',
    readArtifact: 'host:imageGeneration.readArtifact',
    setEnabled: 'host:imageGeneration.setEnabled',
    updateConfiguration: 'host:imageGeneration.updateConfiguration'
  },
  mcp: {
    addServer: 'host:mcp.addServer',
    changed: 'host:mcp.changed',
    deleteServer: 'host:mcp.deleteServer',
    disableServer: 'host:mcp.disableServer',
    enableServer: 'host:mcp.enableServer',
    getServer: 'host:mcp.getServer',
    getStatus: 'host:mcp.getStatus',
    listServers: 'host:mcp.listServers',
    listTools: 'host:mcp.listTools',
    refreshCatalog: 'host:mcp.refreshCatalog',
    requestLaunchAuthorization: 'host:mcp.requestLaunchAuthorization',
    restartServer: 'host:mcp.restartServer',
    selectExecutable: 'host:mcp.selectExecutable',
    selectWorkingDirectory: 'host:mcp.selectWorkingDirectory',
    startServer: 'host:mcp.startServer',
    stopServer: 'host:mcp.stopServer',
    updateServer: 'host:mcp.updateServer'
  },
  office: {
    getStatus: 'host:office.getStatus'
  },
  resources: {
    resolveFavicon: 'host:resources.resolveFavicon'
  },
  search: {
    searchChats: 'host:search.searchChats'
  },
  skills: {
    cancelPreparation: 'host:skills.cancelPreparation',
    cancelSourceResolution: 'host:skills.cancelSourceResolution',
    changed: 'host:skills.changed',
    commitInstallation: 'host:skills.commitInstallation',
    inspectInstallation: 'host:skills.inspectInstallation',
    list: 'host:skills.list',
    listManagement: 'host:skills.listManagement',
    resolveInstallationSource: 'host:skills.resolveInstallationSource',
    selectInstallationDirectory: 'host:skills.selectInstallationDirectory',
    setEnabled: 'host:skills.setEnabled',
    uninstall: 'host:skills.uninstall'
  },
  storage: {
    deleteChatMessages: 'host:storage.deleteChatMessages',
    deleteConversation: 'host:storage.deleteConversation',
    deleteProject: 'host:storage.deleteProject',
    forkConversation: 'host:storage.forkConversation',
    loadAgentPromptPreferences: 'host:storage.loadAgentPromptPreferences',
    loadAttachmentImage: 'host:storage.loadAttachmentImage',
    loadComposerDrafts: 'host:storage.loadComposerDrafts',
    loadConversation: 'host:storage.loadConversation',
    loadConversationMetas: 'host:storage.loadConversationMetas',
    loadConversations: 'host:storage.loadConversations',
    loadImageFile: 'host:storage.loadImageFile',
    loadInputAttachments: 'host:storage.loadInputAttachments',
    loadModelSettings: 'host:storage.loadModelSettings',
    loadProviderProfileUiDescriptors: 'host:storage.loadProviderProfileUiDescriptors',
    loadProjects: 'host:storage.loadProjects',
    loadUiPreferences: 'host:storage.loadUiPreferences',
    revealProjectFile: 'host:storage.revealProjectFile',
    saveAgentPromptPreferences: 'host:storage.saveAgentPromptPreferences',
    saveChatMessageState: 'host:storage.saveChatMessageState',
    saveChatMessageUiState: 'host:storage.saveChatMessageUiState',
    saveComposerDraft: 'host:storage.saveComposerDraft',
    saveConversationMeta: 'host:storage.saveConversationMeta',
    saveModelSettings: 'host:storage.saveModelSettings',
    saveProject: 'host:storage.saveProject',
    saveUiPreferences: 'host:storage.saveUiPreferences',
    selectProfileAvatar: 'host:storage.selectProfileAvatar',
    selectProjectDirectory: 'host:storage.selectProjectDirectory',
    showProjectInFolder: 'host:storage.showProjectInFolder',
    upsertChatMessages: 'host:storage.upsertChatMessages'
  },
  terminal: {
    acknowledgeOutput: 'host:terminal.acknowledgeOutput',
    createSession: 'host:terminal.createSession',
    exit: 'host:terminal.exit',
    killSession: 'host:terminal.killSession',
    output: 'host:terminal.output',
    resizeSession: 'host:terminal.resizeSession',
    writeInput: 'host:terminal.writeInput'
  },
  workspaceFiles: {
    copyPath: 'host:workspaceFiles.copyPath',
    listDirectory: 'host:workspaceFiles.listDirectory',
    readPreview: 'host:workspaceFiles.readPreview',
    revealInFolder: 'host:workspaceFiles.revealInFolder'
  }
} as const
