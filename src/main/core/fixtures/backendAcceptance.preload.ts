import { contextBridge, ipcRenderer } from 'electron'
import { createAgentIpcBridge } from '../../../preload/AgentIpcBridge'
import { createStorageIpcBridge } from '../../../preload/StorageIpcBridge'

const settled = new Set<(result: { channel: string; ok: boolean }) => void>()
const agentIpc = {
  on: ipcRenderer.on.bind(ipcRenderer),
  removeListener: ipcRenderer.removeListener.bind(ipcRenderer),
  invoke: (channel: string, ...args: unknown[]) => {
    const result = ipcRenderer.invoke(channel, ...args)
    const notify = (ok: boolean) => {
      for (const subscriber of settled) subscriber({ channel, ok })
    }
    void result.then(
      (value: unknown) => {
        const response = value as { ok?: boolean; value?: unknown } | null
        notify(response?.ok !== false && response?.value !== false)
      },
      () => notify(false)
    )
    return result
  }
}
contextBridge.exposeInMainWorld('mycopilot', {
  host: {
    agent: createAgentIpcBridge(agentIpc),
    storage: createStorageIpcBridge(ipcRenderer),
    app: { setNativeThemeSource: async () => undefined }
  }
})
contextBridge.exposeInMainWorld('acceptanceTransport', {
  onAgentRpcSettled: (callback: (result: { channel: string; ok: boolean }) => void) => {
    settled.add(callback)
    return () => settled.delete(callback)
  }
})
