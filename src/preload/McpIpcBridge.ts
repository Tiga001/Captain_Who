import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type McpHostApi } from '@mycopilot/host-api'
import type { McpChangedNotification } from '@mycopilot/protocol'

type McpIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

/** Transport-only MCP bridge. It exposes no generic RPC and no direct Tool invocation. */
export function createMcpIpcBridge(ipcRenderer: McpIpcRenderer): McpHostApi {
  return {
    listBuiltinCapabilities: () => ipcRenderer.invoke(HOST_CHANNELS.mcp.listBuiltinCapabilities),
    setBuiltinCapabilityAllowed: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.mcp.setBuiltinCapabilityAllowed, input),
    listServers: () => ipcRenderer.invoke(HOST_CHANNELS.mcp.listServers),
    getServer: (input) => ipcRenderer.invoke(HOST_CHANNELS.mcp.getServer, input),
    addServer: (input) => ipcRenderer.invoke(HOST_CHANNELS.mcp.addServer, input),
    updateServer: (input) => ipcRenderer.invoke(HOST_CHANNELS.mcp.updateServer, input),
    deleteServer: (input) => ipcRenderer.invoke(HOST_CHANNELS.mcp.deleteServer, input),
    requestLaunchAuthorization: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.mcp.requestLaunchAuthorization, input),
    enableServer: (input) => ipcRenderer.invoke(HOST_CHANNELS.mcp.enableServer, input),
    disableServer: (input) => ipcRenderer.invoke(HOST_CHANNELS.mcp.disableServer, input),
    startServer: (input) => ipcRenderer.invoke(HOST_CHANNELS.mcp.startServer, input),
    stopServer: (input) => ipcRenderer.invoke(HOST_CHANNELS.mcp.stopServer, input),
    restartServer: (input) => ipcRenderer.invoke(HOST_CHANNELS.mcp.restartServer, input),
    getStatus: (input) => ipcRenderer.invoke(HOST_CHANNELS.mcp.getStatus, input),
    listTools: (input) => ipcRenderer.invoke(HOST_CHANNELS.mcp.listTools, input),
    refreshCatalog: (input) => ipcRenderer.invoke(HOST_CHANNELS.mcp.refreshCatalog, input),
    selectExecutable: () => ipcRenderer.invoke(HOST_CHANNELS.mcp.selectExecutable),
    selectWorkingDirectory: () => ipcRenderer.invoke(HOST_CHANNELS.mcp.selectWorkingDirectory),
    onChanged: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: McpChangedNotification): void =>
        handler(payload)
      ipcRenderer.on(HOST_CHANNELS.mcp.changed, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.mcp.changed, listener)
    }
  }
}
