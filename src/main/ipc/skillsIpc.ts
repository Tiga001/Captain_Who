import { BrowserWindow } from 'electron'
import type { IpcMainInvokeEvent } from 'electron'
import { captureHostInvocation, HOST_CHANNELS } from '@mycopilot/host-api'
import type { CoreServer } from '../core/coreServer'
import type { TrustedIpcMain } from './trustedIpc'

export function registerSkillsIpc(
  ipcMain: TrustedIpcMain,
  coreServer: CoreServer,
  selectInstallationDirectory: (event: IpcMainInvokeEvent) => Promise<string | null>
): void {
  coreServer.onSkillsChanged((event) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send(HOST_CHANNELS.skills.changed, event)
      }
    }
  })

  ipcMain.handle(HOST_CHANNELS.skills.list, (_event, input) => coreServer.listSkills(input))
  ipcMain.handle(HOST_CHANNELS.skills.selectInstallationDirectory, (event) =>
    selectInstallationDirectory(event)
  )
  ipcMain.handle(HOST_CHANNELS.skills.resolveInstallationSource, (_event, input) =>
    captureHostInvocation(() => coreServer.resolveSkillInstallationSource(input))
  )
  ipcMain.handle(HOST_CHANNELS.skills.cancelSourceResolution, (_event, input) =>
    captureHostInvocation(() => coreServer.cancelSkillSourceResolution(input))
  )
  ipcMain.handle(HOST_CHANNELS.skills.inspectInstallation, (_event, input) =>
    captureHostInvocation(() => coreServer.inspectSkillInstallation(input))
  )
  ipcMain.handle(HOST_CHANNELS.skills.commitInstallation, (_event, input) =>
    captureHostInvocation(() => coreServer.commitSkillInstallation(input))
  )
  ipcMain.handle(HOST_CHANNELS.skills.cancelPreparation, (_event, input) =>
    captureHostInvocation(() => coreServer.cancelSkillPreparation(input))
  )
  ipcMain.handle(HOST_CHANNELS.skills.listManagement, (_event, input) =>
    captureHostInvocation(() => coreServer.listSkillManagement(input))
  )
  ipcMain.handle(HOST_CHANNELS.skills.setEnabled, (_event, input) =>
    captureHostInvocation(() => coreServer.setSkillEnabled(input))
  )
  ipcMain.handle(HOST_CHANNELS.skills.uninstall, (_event, input) =>
    captureHostInvocation(() => coreServer.uninstallSkill(input))
  )
}
