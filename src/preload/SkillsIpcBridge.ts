import type { IpcRenderer } from 'electron'
import type { IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type SkillsHostApi } from '@mycopilot/host-api'
import type { SkillsChangedNotification } from '@mycopilot/protocol'

type SkillsIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

/** CoreServer validates protocol payloads; preload deliberately remains transport-only. */
export function createSkillsIpcBridge(ipcRenderer: SkillsIpcRenderer): SkillsHostApi {
  return {
    list: (input) => ipcRenderer.invoke(HOST_CHANNELS.skills.list, input),
    selectInstallationDirectory: () =>
      ipcRenderer.invoke(HOST_CHANNELS.skills.selectInstallationDirectory),
    resolveInstallationSource: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.skills.resolveInstallationSource, input),
    cancelSourceResolution: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.skills.cancelSourceResolution, input),
    inspectInstallation: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.skills.inspectInstallation, input),
    commitInstallation: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.skills.commitInstallation, input),
    cancelPreparation: (input) => ipcRenderer.invoke(HOST_CHANNELS.skills.cancelPreparation, input),
    listManagement: (input) => ipcRenderer.invoke(HOST_CHANNELS.skills.listManagement, input),
    setEnabled: (input) => ipcRenderer.invoke(HOST_CHANNELS.skills.setEnabled, input),
    uninstall: (input) => ipcRenderer.invoke(HOST_CHANNELS.skills.uninstall, input),
    onChanged: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: SkillsChangedNotification): void =>
        handler(payload)
      ipcRenderer.on(HOST_CHANNELS.skills.changed, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.skills.changed, listener)
    }
  }
}
