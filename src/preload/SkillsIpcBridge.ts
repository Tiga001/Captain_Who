import type { IpcRenderer } from 'electron'
import type { IpcRendererEvent } from 'electron'
import type { SkillsHostApi } from '@mycopilot/host-api'
import type { SkillsChangedNotification } from '@mycopilot/protocol'

const SKILLS_LIST_CHANNEL = 'host:skills.list'
const SKILLS_SELECT_INSTALLATION_DIRECTORY_CHANNEL = 'host:skills.selectInstallationDirectory'
const SKILLS_RESOLVE_INSTALLATION_SOURCE_CHANNEL = 'host:skills.resolveInstallationSource'
const SKILLS_CANCEL_SOURCE_RESOLUTION_CHANNEL = 'host:skills.cancelSourceResolution'
const SKILLS_INSPECT_INSTALLATION_CHANNEL = 'host:skills.inspectInstallation'
const SKILLS_COMMIT_INSTALLATION_CHANNEL = 'host:skills.commitInstallation'
const SKILLS_CANCEL_PREPARATION_CHANNEL = 'host:skills.cancelPreparation'
const SKILLS_LIST_MANAGEMENT_CHANNEL = 'host:skills.listManagement'
const SKILLS_SET_ENABLED_CHANNEL = 'host:skills.setEnabled'
const SKILLS_UNINSTALL_CHANNEL = 'host:skills.uninstall'
const SKILLS_CHANGED_CHANNEL = 'host:skills.changed'

type SkillsIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

/** CoreServer validates protocol payloads; preload deliberately remains transport-only. */
export function createSkillsIpcBridge(ipcRenderer: SkillsIpcRenderer): SkillsHostApi {
  return {
    list: (input) => ipcRenderer.invoke(SKILLS_LIST_CHANNEL, input),
    selectInstallationDirectory: () =>
      ipcRenderer.invoke(SKILLS_SELECT_INSTALLATION_DIRECTORY_CHANNEL),
    resolveInstallationSource: (input) =>
      ipcRenderer.invoke(SKILLS_RESOLVE_INSTALLATION_SOURCE_CHANNEL, input),
    cancelSourceResolution: (input) =>
      ipcRenderer.invoke(SKILLS_CANCEL_SOURCE_RESOLUTION_CHANNEL, input),
    inspectInstallation: (input) => ipcRenderer.invoke(SKILLS_INSPECT_INSTALLATION_CHANNEL, input),
    commitInstallation: (input) => ipcRenderer.invoke(SKILLS_COMMIT_INSTALLATION_CHANNEL, input),
    cancelPreparation: (input) => ipcRenderer.invoke(SKILLS_CANCEL_PREPARATION_CHANNEL, input),
    listManagement: (input) => ipcRenderer.invoke(SKILLS_LIST_MANAGEMENT_CHANNEL, input),
    setEnabled: (input) => ipcRenderer.invoke(SKILLS_SET_ENABLED_CHANNEL, input),
    uninstall: (input) => ipcRenderer.invoke(SKILLS_UNINSTALL_CHANNEL, input),
    onChanged: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: SkillsChangedNotification): void =>
        handler(payload)
      ipcRenderer.on(SKILLS_CHANGED_CHANNEL, listener)
      return () => ipcRenderer.removeListener(SKILLS_CHANGED_CHANNEL, listener)
    }
  }
}
