import { contextBridge, ipcRenderer } from 'electron'
import { HOST_CHANNELS, type HostApi } from '@mycopilot/host-api'
import { createAgentIpcBridge } from './AgentIpcBridge'
import { createAutomationIpcBridge } from './AutomationIpcBridge'
import { createAppIpcBridge } from './AppIpcBridge'
import { createBrowserIpcBridge } from './BrowserIpcBridge'
import { createGitIpcBridge } from './GitIpcBridge'
import { createSkillsIpcBridge } from './SkillsIpcBridge'
import { createOfficeIpcBridge } from './OfficeIpcBridge'
import { createImageGenerationIpcBridge } from './ImageGenerationIpcBridge'
import { createMcpIpcBridge } from './McpIpcBridge'
import { createNotificationIpcBridge } from './NotificationIpcBridge'
import { createStorageIpcBridge } from './StorageIpcBridge'
import { createTerminalIpcBridge } from './TerminalIpcBridge'
import { createWorkspaceFilesIpcBridge } from './WorkspaceFilesIpcBridge'

const host: HostApi = {
  core: {
    ping: (input) => ipcRenderer.invoke(HOST_CHANNELS.core.ping, input)
  },
  app: createAppIpcBridge(ipcRenderer),
  agent: createAgentIpcBridge(ipcRenderer),
  automations: createAutomationIpcBridge(ipcRenderer),
  attachments: {
    selectInputAttachments: (request) =>
      ipcRenderer.invoke(HOST_CHANNELS.attachments.selectInputAttachments, request)
  },
  browser: createBrowserIpcBridge(ipcRenderer),
  git: createGitIpcBridge(ipcRenderer),
  imageGeneration: createImageGenerationIpcBridge(ipcRenderer),
  mcp: createMcpIpcBridge(ipcRenderer),
  notifications: createNotificationIpcBridge(ipcRenderer),
  office: createOfficeIpcBridge(ipcRenderer),
  resources: {
    resolveFavicon: (input) => ipcRenderer.invoke(HOST_CHANNELS.resources.resolveFavicon, input)
  },
  search: {
    searchChats: (input) => ipcRenderer.invoke(HOST_CHANNELS.search.searchChats, input)
  },
  skills: createSkillsIpcBridge(ipcRenderer),
  storage: createStorageIpcBridge(ipcRenderer),
  terminal: createTerminalIpcBridge(ipcRenderer),
  workspaceFiles: createWorkspaceFilesIpcBridge(ipcRenderer)
}

if (process.contextIsolated) {
  contextBridge.exposeInMainWorld('mycopilot', { host })
} else {
  throw new Error('Captain Who preload requires context isolation')
}
