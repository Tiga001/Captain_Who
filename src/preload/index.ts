import { contextBridge, ipcRenderer } from 'electron'
import { HOST_CHANNELS, type HostApi } from '@mycopilot/host-api'
import { createAgentIpcBridge } from './AgentIpcBridge'
import { createAuthIpcBridge } from './AuthIpcBridge'
import { createLicenseIpcBridge } from './LicenseIpcBridge'
import { createUpdateIpcBridge } from './UpdateIpcBridge'
import { createAutomationIpcBridge } from './AutomationIpcBridge'
import { createAppIpcBridge } from './AppIpcBridge'
import { createBrowserIpcBridge } from './BrowserIpcBridge'
import { createGitIpcBridge } from './GitIpcBridge'
import { createSkillsIpcBridge } from './SkillsIpcBridge'
import { createOfficeIpcBridge } from './OfficeIpcBridge'
import { createImageGenerationIpcBridge } from './ImageGenerationIpcBridge'
import { createMcpIpcBridge } from './McpIpcBridge'
import { createNotificationIpcBridge } from './NotificationIpcBridge'
import { createHumanInteractionIpcBridge } from './HumanInteractionIpcBridge'
import { createStorageIpcBridge } from './StorageIpcBridge'
import { createTerminalIpcBridge } from './TerminalIpcBridge'
import { createWorkspaceFilesIpcBridge } from './WorkspaceFilesIpcBridge'

const host: HostApi = {
  updates: createUpdateIpcBridge(ipcRenderer),
  license: createLicenseIpcBridge(ipcRenderer),
  auth: createAuthIpcBridge(ipcRenderer),
  core: {
    ping: (input) => ipcRenderer.invoke(HOST_CHANNELS.core.ping, input)
  },
  app: createAppIpcBridge(ipcRenderer),
  agent: createAgentIpcBridge(ipcRenderer),
  automations: createAutomationIpcBridge(ipcRenderer),
  attachments: {
    selectInputAttachments: (request) =>
      ipcRenderer.invoke(HOST_CHANNELS.attachments.selectInputAttachments, request),
    beginImport: (input) => ipcRenderer.invoke(HOST_CHANNELS.attachments.beginImport, input),
    appendImport: (input) => ipcRenderer.invoke(HOST_CHANNELS.attachments.appendImport, input),
    finishImport: (input) => ipcRenderer.invoke(HOST_CHANNELS.attachments.finishImport, input),
    cancelImport: (input) => ipcRenderer.invoke(HOST_CHANNELS.attachments.cancelImport, input),
    retryInputAttachment: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.attachments.retryInputAttachment, input),
    loadPreview: (input) => ipcRenderer.invoke(HOST_CHANNELS.attachments.loadPreview, input),
    onImportProgress: (listener) => {
      const channel = HOST_CHANNELS.attachments.importProgress
      const handle: Parameters<typeof ipcRenderer.on>[1] = (_event, progress) => listener(progress)
      ipcRenderer.on(channel, handle)
      return () => {
        ipcRenderer.removeListener(channel, handle)
      }
    }
  },
  browser: createBrowserIpcBridge(ipcRenderer),
  git: createGitIpcBridge(ipcRenderer),
  humanInteraction: createHumanInteractionIpcBridge(ipcRenderer),
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
