import { BrowserWindow, type WebContents } from 'electron'
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
  parseAutomationOpenRequest,
  parseAutomationResync,
  parseAutomationRun,
  parseAutomationRunNowInput,
  parseAutomationRunsListInput,
  parseAutomationRunsListOutput,
  parseAutomationSetEnabledInput,
  parseAutomationTask,
  parseAutomationUpdateInput
} from '@mycopilot/protocol'
import type { AutomationOpenRequest, AutomationResync } from '@mycopilot/protocol'
import type { CoreServer } from '../core/coreServer'
import type { TrustedIpcMain } from './trustedIpc'

export interface AutomationIpcRegistration {
  (): void
  beginShutdown(): void
}

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

export function registerAutomationIpc(
  ipcMain: TrustedIpcMain,
  coreServer: CoreServer,
  assertCanStartTurn: () => void | Promise<void> = () => {
    throw new Error('ACCOUNT_LOGIN_REQUIRED')
  }
): AutomationIpcRegistration {
  let latestResync: AutomationResync | null = null
  let pendingOpenRequest: AutomationOpenRequest | null = null
  const readyRenderers: WebContents[] = []

  const sendOpenRequest = (value: AutomationOpenRequest): void => {
    const request = parseAutomationOpenRequest(value)
    while (readyRenderers.length > 0) {
      const target = readyRenderers.at(-1)
      if (!target || target.isDestroyed()) {
        readyRenderers.pop()
        continue
      }
      const window = BrowserWindow.fromWebContents(target)
      if (window && !window.isDestroyed()) {
        try {
          if (window.isMinimized()) window.restore()
          window.show()
          window.focus()
        } catch {
          // Navigation delivery is still useful if native focus loses a close race.
        }
      }
      try {
        target.send(HOST_CHANNELS.automations.openRequested, request)
        return
      } catch {
        readyRenderers.pop()
        console.warn('Failed to deliver Automation notification navigation to a window')
      }
    }
    pendingOpenRequest = request
  }
  const unsubscribeEvent = coreServer.onAutomationEvent((value) => {
    const event = parseAutomationEvent(value)
    broadcast(HOST_CHANNELS.automations.event, event)
  })
  const unsubscribeResync = coreServer.onAutomationResync((value) => {
    latestResync = parseAutomationResync(value)
    broadcast(HOST_CHANNELS.automations.resync, latestResync)
  })
  ipcMain.on(HOST_CHANNELS.automations.resyncReady, (event) => {
    const existingIndex = readyRenderers.indexOf(event.sender)
    if (existingIndex >= 0) readyRenderers.splice(existingIndex, 1)
    readyRenderers.push(event.sender)
    if (latestResync !== null && !event.sender.isDestroyed()) {
      event.sender.send(HOST_CHANNELS.automations.resync, latestResync)
    }
    if (pendingOpenRequest !== null && !event.sender.isDestroyed()) {
      const request = pendingOpenRequest
      pendingOpenRequest = null
      sendOpenRequest(request)
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
    captureHostInvocation(async () => {
      const parsed = parseAutomationRunNowInput(input)
      await assertCanStartTurn()
      return parseAutomationRun(await coreServer.runAutomationNow(parsed))
    })
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

  let disposed = false
  const beginShutdown = (): void => undefined
  const dispose = (): void => {
    if (disposed) return
    disposed = true
    readyRenderers.length = 0
    pendingOpenRequest = null
    unsubscribeEvent()
    unsubscribeResync()
  }
  dispose.beginShutdown = beginShutdown
  return dispose
}
