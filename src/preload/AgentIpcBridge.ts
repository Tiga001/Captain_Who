import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type AgentHostApi } from '@mycopilot/host-api'
import type { AgentEvent } from '@mycopilot/protocol'

type AgentIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

export function createAgentIpcBridge(ipcRenderer: AgentIpcRenderer): AgentHostApi {
  return {
    startConversationTurn: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.agent.startConversationTurn, input),
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
    clearUsageRecords: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.clearUsageRecords, input),
    readFileDraft: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.readFileDraft, input),
    getFileWriteDiff: (input) => ipcRenderer.invoke(HOST_CHANNELS.agent.getFileWriteDiff, input),
    onEvent: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: AgentEvent): void => handler(payload)
      ipcRenderer.on(HOST_CHANNELS.agent.event, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.agent.event, listener)
    }
  }
}
