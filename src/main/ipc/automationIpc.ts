import { BrowserWindow } from 'electron'
import { captureHostInvocation, HOST_CHANNELS } from '@mycopilot/host-api'
import {
  parseAutomationAttentionAcknowledgeInput,
  parseAutomationAttentionAcknowledgeOutput,
  parseAutomationAttentionSummaryInput,
  parseAutomationAttentionSummaryOutput,
  parseAutomationCreateInput,
  parseAutomationDeleteInput,
  parseAutomationDeleteOutput,
  parseAutomationEvent,
  parseAutomationGetInput,
  parseAutomationListInput,
  parseAutomationListOutput,
  parseAutomationResync,
  parseAutomationRun,
  parseAutomationRunNowInput,
  parseAutomationRunsListInput,
  parseAutomationRunsListOutput,
  parseAutomationSetEnabledInput,
  parseAutomationTask,
  parseAutomationUpdateInput
} from '@mycopilot/protocol'
import type { AutomationResync } from '@mycopilot/protocol'
import type { CoreServer } from '../core/coreServer'
import type { TrustedIpcMain } from './trustedIpc'

function broadcast(channel: string, payload: unknown): void {
  for (const window of BrowserWindow.getAllWindows()) {
    if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
      try {
        window.webContents.send(channel, payload)
      } catch {
        console.warn('Failed to broadcast Automation notification to a window')
      }
    }
  }
}

export function registerAutomationIpc(ipcMain: TrustedIpcMain, coreServer: CoreServer): () => void {
  let latestResync: AutomationResync | null = null
  const unsubscribeEvent = coreServer.onAutomationEvent((value) => {
    broadcast(HOST_CHANNELS.automations.event, parseAutomationEvent(value))
  })
  const unsubscribeResync = coreServer.onAutomationResync((value) => {
    latestResync = parseAutomationResync(value)
    broadcast(HOST_CHANNELS.automations.resync, latestResync)
  })
  ipcMain.on(HOST_CHANNELS.automations.resyncReady, (event) => {
    if (latestResync !== null && !event.sender.isDestroyed()) {
      event.sender.send(HOST_CHANNELS.automations.resync, latestResync)
    }
  })

  ipcMain.handle(HOST_CHANNELS.automations.list, (_event, input) =>
    captureHostInvocation(async () =>
      parseAutomationListOutput(await coreServer.listAutomations(parseAutomationListInput(input)))
    )
  )
  ipcMain.handle(HOST_CHANNELS.automations.get, (_event, input) =>
    captureHostInvocation(async () =>
      parseAutomationTask(await coreServer.getAutomation(parseAutomationGetInput(input)))
    )
  )
  ipcMain.handle(HOST_CHANNELS.automations.create, (_event, input) =>
    captureHostInvocation(async () =>
      parseAutomationTask(await coreServer.createAutomation(parseAutomationCreateInput(input)))
    )
  )
  ipcMain.handle(HOST_CHANNELS.automations.update, (_event, input) =>
    captureHostInvocation(async () =>
      parseAutomationTask(await coreServer.updateAutomation(parseAutomationUpdateInput(input)))
    )
  )
  ipcMain.handle(HOST_CHANNELS.automations.setEnabled, (_event, input) =>
    captureHostInvocation(async () =>
      parseAutomationTask(
        await coreServer.setAutomationEnabled(parseAutomationSetEnabledInput(input))
      )
    )
  )
  ipcMain.handle(HOST_CHANNELS.automations.runNow, (_event, input) =>
    captureHostInvocation(async () =>
      parseAutomationRun(await coreServer.runAutomationNow(parseAutomationRunNowInput(input)))
    )
  )
  ipcMain.handle(HOST_CHANNELS.automations.delete, (_event, input) =>
    captureHostInvocation(async () =>
      parseAutomationDeleteOutput(
        await coreServer.deleteAutomation(parseAutomationDeleteInput(input))
      )
    )
  )
  ipcMain.handle(HOST_CHANNELS.automations.listRuns, (_event, input) =>
    captureHostInvocation(async () =>
      parseAutomationRunsListOutput(
        await coreServer.listAutomationRuns(parseAutomationRunsListInput(input))
      )
    )
  )
  ipcMain.handle(HOST_CHANNELS.automations.attentionSummary, (_event, input) =>
    captureHostInvocation(async () =>
      parseAutomationAttentionSummaryOutput(
        await coreServer.getAutomationAttentionSummary(parseAutomationAttentionSummaryInput(input))
      )
    )
  )
  ipcMain.handle(HOST_CHANNELS.automations.acknowledgeAttention, (_event, input) =>
    captureHostInvocation(async () =>
      parseAutomationAttentionAcknowledgeOutput(
        await coreServer.acknowledgeAutomationAttention(
          parseAutomationAttentionAcknowledgeInput(input)
        )
      )
    )
  )

  return () => {
    unsubscribeEvent()
    unsubscribeResync()
  }
}
