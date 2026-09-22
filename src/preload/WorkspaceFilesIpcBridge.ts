import type { IpcRenderer } from 'electron'
import { HOST_CHANNELS, type WorkspaceFilesHostApi } from '@mycopilot/host-api'

type WorkspaceFilesIpcRenderer = Pick<IpcRenderer, 'invoke'>

export function createWorkspaceFilesIpcBridge(
  ipcRenderer: WorkspaceFilesIpcRenderer
): WorkspaceFilesHostApi {
  return {
    copyPath: (input) => ipcRenderer.invoke(HOST_CHANNELS.workspaceFiles.copyPath, input),
    listDirectory: (input) => ipcRenderer.invoke(HOST_CHANNELS.workspaceFiles.listDirectory, input),
    searchMentions: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.workspaceFiles.searchMentions, input),
    readPreview: (input) => ipcRenderer.invoke(HOST_CHANNELS.workspaceFiles.readPreview, input),
    revealInFolder: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.workspaceFiles.revealInFolder, input)
  }
}
