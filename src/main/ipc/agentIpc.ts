import { BrowserWindow } from 'electron'
import {
  captureHostInvocation,
  HOST_CHANNELS,
  type HostInvocationResult
} from '@mycopilot/host-api'
import { parseAgentApproveActionRequest } from '@mycopilot/protocol'
import type { CoreServer } from '../core/coreServer'
import type { TrustedIpcMain } from './trustedIpc'

export function registerAgentIpc(ipcMain: TrustedIpcMain, coreServer: CoreServer): void {
  coreServer.onAgentEvent((event) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send(HOST_CHANNELS.agent.event, event)
      }
    }
  })

  coreServer.onManualContextCompaction?.((event) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send(HOST_CHANNELS.agent.manualContextCompaction, event)
      }
    }
  })

  coreServer.onProviderTransition((event) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send(HOST_CHANNELS.agent.providerTransition, event)
      }
    }
  })

  coreServer.onCollaborationEvent?.((event) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send(HOST_CHANNELS.agent.collaborationEvent, event)
      }
    }
  })

  coreServer.onCollaborationObserverEvent?.((event) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send(HOST_CHANNELS.agent.collaborationObserverEvent, event)
      }
    }
  })

  coreServer.onCollaborationResync?.((event) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send(HOST_CHANNELS.agent.collaborationResync, event)
      }
    }
  })

  ipcMain.handle(HOST_CHANNELS.agent.collaborationGetTree, (_event, input) =>
    captureHostInvocation(() => coreServer.getCollaborationTree(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.collaborationGetAgent, (_event, input) =>
    captureHostInvocation(() => coreServer.getCollaborationAgent(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.collaborationLocateConversation, (_event, input) =>
    captureHostInvocation(() => coreServer.locateCollaborationConversation(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.collaborationLoadObserverConversation, (_event, input) =>
    captureHostInvocation(() => coreServer.loadCollaborationObserverConversation(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.collaborationListEvents, (_event, input) =>
    captureHostInvocation(() => coreServer.listCollaborationEvents(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.collaborationTemplateList, (_event, input) =>
    captureHostInvocation(() => coreServer.listAgentTemplates(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.collaborationTemplateCreate, (_event, input) =>
    captureHostInvocation(() => coreServer.createAgentTemplate(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.collaborationTemplateUpdate, (_event, input) =>
    captureHostInvocation(() => coreServer.updateAgentTemplate(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.collaborationTemplateSetEnabled, (_event, input) =>
    captureHostInvocation(() => coreServer.setAgentTemplateEnabled(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.collaborationTemplateSetProjectAssignment, (_event, input) =>
    captureHostInvocation(() => coreServer.setAgentTemplateProjectAssignment(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.collaborationTemplateDelete, (_event, input) =>
    captureHostInvocation(() => coreServer.deleteAgentTemplate(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.collaborationApprovalList, (_event, input) =>
    captureHostInvocation(() => coreServer.listCollaborationApprovals(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.collaborationApprovalDecide, (_event, input) =>
    captureHostInvocation(() => coreServer.decideCollaborationApproval(input))
  )

  ipcMain.handle(HOST_CHANNELS.agent.startManualContextCompaction, (_event, input) =>
    captureProviderTransitionInvocation(
      () => coreServer.startManualContextCompaction(input),
      'Unable to complete this compaction request. Please try again.'
    )
  )
  ipcMain.handle(HOST_CHANNELS.agent.getManualContextCompactionStatus, (_event, input) =>
    captureProviderTransitionInvocation(
      () => coreServer.getManualContextCompactionStatus(input),
      'Unable to complete this compaction request. Please try again.'
    )
  )
  ipcMain.handle(HOST_CHANNELS.agent.cancelManualContextCompaction, (_event, input) =>
    captureProviderTransitionInvocation(
      () => coreServer.cancelManualContextCompaction(input),
      'Unable to complete this compaction request. Please try again.'
    )
  )
  ipcMain.handle(HOST_CHANNELS.agent.preflightProviderTransition, (_event, input) =>
    captureProviderTransitionInvocation(
      () => coreServer.preflightProviderTransition(input),
      'Unable to check this model switch. Please try again.'
    )
  )
  ipcMain.handle(HOST_CHANNELS.agent.startProviderTransition, (_event, input) =>
    captureProviderTransitionInvocation(
      () => coreServer.startProviderTransition(input),
      'The model-switch check expired. Please try again.'
    )
  )
  ipcMain.handle(HOST_CHANNELS.agent.getProviderTransitionStatus, (_event, input) =>
    captureProviderTransitionInvocation(
      () => coreServer.getProviderTransitionStatus(input),
      'Unable to restore the model-switch status. Please try again.'
    )
  )
  ipcMain.handle(HOST_CHANNELS.agent.startConversationTurn, (_event, input) =>
    captureHostInvocation(() => coreServer.startConversationTurn(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.rewriteConversationTurn, (_event, input) =>
    captureHostInvocation(() => coreServer.rewriteConversationTurn(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.getContextWindowSnapshot, (_event, input) =>
    captureHostInvocation(() => coreServer.getContextWindowSnapshot(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.listCommandSessions, (_event, input) =>
    captureHostInvocation(() => coreServer.listCommandSessions(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.getCommandSession, (_event, input) =>
    captureHostInvocation(() => coreServer.getCommandSession(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.steerRun, (_event, input) => coreServer.steerRun(input))
  ipcMain.handle(HOST_CHANNELS.agent.cancelRun, (_event, input) => coreServer.cancelRun(input))
  ipcMain.handle(HOST_CHANNELS.agent.listPendingActions, () => coreServer.listPendingActions())
  ipcMain.handle(HOST_CHANNELS.agent.approveAction, (_event, input) =>
    coreServer.approveAction(parseAgentApproveActionRequest(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.rejectAction, (_event, input) =>
    coreServer.rejectAction(input)
  )
  ipcMain.handle(HOST_CHANNELS.agent.cancelAction, (_event, input) =>
    coreServer.cancelAction(input)
  )
  ipcMain.handle(HOST_CHANNELS.agent.getUsageSummary, (_event, input) =>
    coreServer.getUsageSummary(input)
  )
  ipcMain.handle(HOST_CHANNELS.agent.clearUsageRecords, (_event, input) =>
    coreServer.clearUsageRecords(input)
  )
  ipcMain.handle(HOST_CHANNELS.agent.readFileChange, (_event, input) =>
    coreServer.readFileChange(input)
  )
  ipcMain.handle(HOST_CHANNELS.agent.getFileChangeDiff, (_event, input) =>
    coreServer.getFileChangeDiff(input)
  )
  ipcMain.handle(HOST_CHANNELS.agent.getFileChangeHistoryDiff, (_event, input) =>
    coreServer.getFileChangeHistoryDiff(input)
  )
}

/**
 * Provider-transition failures must never forward arbitrary Core data or diagnostics to Renderer.
 * Typed asynchronous failures travel through the strictly parsed operation notification instead.
 */
async function captureProviderTransitionInvocation<T>(
  operation: () => Promise<T>,
  safeErrorMessage: string
): Promise<HostInvocationResult<T>> {
  const result = await captureHostInvocation(operation)
  if (result.ok) return result
  return {
    ok: false,
    error: { message: safeErrorMessage }
  }
}
