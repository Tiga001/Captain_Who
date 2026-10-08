import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type AgentHostApi } from '@mycopilot/host-api'
import {
  parseAgentCollaborationSettings,
  parseWorkflowRuntimeSnapshot,
  parseAgentPromptPreferencesChanged
} from '@mycopilot/protocol'
import type {
  AgentEvent,
  AgentObserverEventEnvelope,
  CollaborationEventEnvelope,
  CollaborationResyncEnvelope
} from '@mycopilot/protocol'

type AgentIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

export function createAgentIpcBridge(ipcRenderer: AgentIpcRenderer): AgentHostApi {
  // One validated projection per incoming IPC message, regardless of the number of panels.
  // Keep no last-value cache: subscribing after navigation/auth changes must not replay old data.
  const runtimeSubscribers = new Set<Parameters<AgentHostApi['onWorkflowRuntimeChanged']>[0]>()
  const runtimeListener = (_event: IpcRendererEvent, payload: unknown): void => {
    let snapshot: ReturnType<typeof parseWorkflowRuntimeSnapshot>
    try {
      snapshot = parseWorkflowRuntimeSnapshot(payload)
    } catch {
      return
    }
    for (const subscriber of [...runtimeSubscribers]) {
      try {
        subscriber(snapshot)
      } catch {
        // A broken panel must not prevent terminal/unread/preference subscribers from updating.
        console.error('Organization runtime subscriber failed')
      }
    }
  }
  return {
    onPromptPreferencesChanged: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: unknown): void => {
        let event: ReturnType<typeof parseAgentPromptPreferencesChanged>
        try {
          event = parseAgentPromptPreferencesChanged(payload)
        } catch {
          return
        }
        handler(event)
      }
      ipcRenderer.on(HOST_CHANNELS.agent.promptPreferencesChanged, listener)
      return () =>
        ipcRenderer.removeListener(HOST_CHANNELS.agent.promptPreferencesChanged, listener)
    },
    getCollaborationSettings: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationGetSettings, input),
    updateCollaborationSettings: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationUpdateSettings, input),
    onCollaborationSettingsChanged: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: unknown): void => {
        let settings: ReturnType<typeof parseAgentCollaborationSettings>
        try {
          settings = parseAgentCollaborationSettings(payload)
        } catch {
          return
        }
        handler(settings)
      }
      ipcRenderer.on(HOST_CHANNELS.agent.collaborationSettingsChanged, listener)
      return () =>
        ipcRenderer.removeListener(HOST_CHANNELS.agent.collaborationSettingsChanged, listener)
    },
    getCollaborationTree: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationGetTree, input),
    getCollaborationAgent: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationGetAgent, input),
    locateCollaborationConversation: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationLocateConversation, input),
    loadCollaborationObserverConversation: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationLoadObserverConversation, input),
    listCollaborationEvents: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationListEvents, input),
    requestWorkflows: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.workflows, input),
    onWorkflowRuntimeChanged: (handler) => {
      const subscriber: typeof handler = (snapshot) => handler(snapshot)
      if (!runtimeSubscribers.size)
        ipcRenderer.on(HOST_CHANNELS.agent.workflowRuntimeChanged, runtimeListener)
      runtimeSubscribers.add(subscriber)
      return () => {
        if (runtimeSubscribers.delete(subscriber) && !runtimeSubscribers.size)
          ipcRenderer.removeListener(HOST_CHANNELS.agent.workflowRuntimeChanged, runtimeListener)
      }
    },
    listAgentTemplates: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationTemplateList, input),
    createAgentTemplate: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationTemplateCreate, input),
    updateAgentTemplate: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationTemplateUpdate, input),
    setAgentTemplateEnabled: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationTemplateSetEnabled, input),
    setAgentTemplateProjectAssignment: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationTemplateSetProjectAssignment, input),
    deleteAgentTemplate: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationTemplateDelete, input),
    listCollaborationApprovals: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationApprovalList, input),
    decideCollaborationApproval: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.collaborationApprovalDecide, input),
    onCollaborationEvent: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: CollaborationEventEnvelope): void =>
        handler(payload)
      ipcRenderer.on(HOST_CHANNELS.agent.collaborationEvent, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.agent.collaborationEvent, listener)
    },
    onCollaborationObserverEvent: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: AgentObserverEventEnvelope): void =>
        handler(payload)
      ipcRenderer.on(HOST_CHANNELS.agent.collaborationObserverEvent, listener)
      return () =>
        ipcRenderer.removeListener(HOST_CHANNELS.agent.collaborationObserverEvent, listener)
    },
    onCollaborationResync: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: CollaborationResyncEnvelope): void =>
        handler(payload)
      ipcRenderer.on(HOST_CHANNELS.agent.collaborationResync, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.agent.collaborationResync, listener)
    },
    startManualContextCompaction: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.startManualContextCompaction, input),
    getManualContextCompactionStatus: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.getManualContextCompactionStatus, input),
    cancelManualContextCompaction: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.cancelManualContextCompaction, input),
    onManualContextCompaction: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: Parameters<typeof handler>[0]): void =>
        handler(payload)
      ipcRenderer.on(HOST_CHANNELS.agent.manualContextCompaction, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.agent.manualContextCompaction, listener)
    },
    preflightProviderTransition: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.preflightProviderTransition, input),
    startProviderTransition: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.startProviderTransition, input),
    getProviderTransitionStatus: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.getProviderTransitionStatus, input),
    startConversationTurn: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.startConversationTurn, input),
    continueConversationTurn: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.continueConversationTurn, input),
    rewriteConversationTurn: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.rewriteConversationTurn, input),
    getContextWindowSnapshot: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.getContextWindowSnapshot, input),
    listCommandSessions: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.listCommandSessions, input),
    getCommandSession: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.getCommandSession, input),
    steerRun: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.steerRun, input),
    cancelRun: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.cancelRun, input),
    listPendingActions: () => ipcRenderer.invoke(HOST_CHANNELS.agent.listPendingActions),
    approveAction: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.approveAction, input),
    rejectAction: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.rejectAction, input),
    cancelAction: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.cancelAction, input),
    getUsageSummary: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.getUsageSummary, input),
    getUsageDashboard: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.getUsageDashboard, input),
    getLocalTokenUsage: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.getLocalTokenUsage, input),
    clearUsageRecords: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.clearUsageRecords, input),
    readFileChange: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.readFileChange, input),
    getFileChangeDiff: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.getFileChangeDiff, input),
    getFileChangeHistoryDiff: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.getFileChangeHistoryDiff, input),
    onProviderTransition: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: Parameters<typeof handler>[0]): void =>
        handler(payload)
      ipcRenderer.on(HOST_CHANNELS.agent.providerTransition, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.agent.providerTransition, listener)
    },
    onEvent: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: AgentEvent): void => handler(payload)
      ipcRenderer.on(HOST_CHANNELS.agent.event, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.agent.event, listener)
    }
  }
}
