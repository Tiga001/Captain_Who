import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type AutomationsHostApi } from '@mycopilot/host-api'
import type { AutomationEvent, AutomationResync } from '@mycopilot/protocol'

type AutomationIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener' | 'send'>

/** Context-isolated transport only; Main and Core own strict protocol validation. */
export function createAutomationIpcBridge(ipcRenderer: AutomationIpcRenderer): AutomationsHostApi {
  return {
    list: (input) => ipcRenderer.invoke(HOST_CHANNELS.automations.list, input),
    get: (input) => ipcRenderer.invoke(HOST_CHANNELS.automations.get, input),
    create: (input) => ipcRenderer.invoke(HOST_CHANNELS.automations.create, input),
    update: (input) => ipcRenderer.invoke(HOST_CHANNELS.automations.update, input),
    setEnabled: (input) => ipcRenderer.invoke(HOST_CHANNELS.automations.setEnabled, input),
    runNow: (input) => ipcRenderer.invoke(HOST_CHANNELS.automations.runNow, input),
    delete: (input) => ipcRenderer.invoke(HOST_CHANNELS.automations.delete, input),
    listRuns: (input) => ipcRenderer.invoke(HOST_CHANNELS.automations.listRuns, input),
    attentionSummary: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.automations.attentionSummary, input),
    acknowledgeAttention: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.automations.acknowledgeAttention, input),
    onEvent: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: AutomationEvent): void =>
        handler(payload)
      ipcRenderer.on(HOST_CHANNELS.automations.event, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.automations.event, listener)
    },
    onResync: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: AutomationResync): void =>
        handler(payload)
      ipcRenderer.on(HOST_CHANNELS.automations.resync, listener)
      ipcRenderer.send(HOST_CHANNELS.automations.resyncReady)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.automations.resync, listener)
    }
  }
}
