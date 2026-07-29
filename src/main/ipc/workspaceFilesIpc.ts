import { clipboard, shell } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import type { WorkspaceFilesService } from '../workspaceFiles/WorkspaceFilesService'
import type { TrustedIpcMain } from './trustedIpc'

export function registerWorkspaceFilesIpc(
  ipcMain: TrustedIpcMain,
  workspaceFilesService: WorkspaceFilesService
): void {
  ipcMain.handle(HOST_CHANNELS.workspaceFiles.copyPath, async (_event, input) => {
    clipboard.writeText(await workspaceFilesService.resolvePathForReveal(input))
  })
  ipcMain.handle(HOST_CHANNELS.workspaceFiles.listDirectory, (_event, input) =>
    workspaceFilesService.listDirectory(input)
  )
  ipcMain.handle(HOST_CHANNELS.workspaceFiles.readPreview, (_event, input) =>
    workspaceFilesService.readPreview(input)
  )
  ipcMain.handle(HOST_CHANNELS.workspaceFiles.revealInFolder, async (_event, input) => {
    shell.showItemInFolder(await workspaceFilesService.resolvePathForReveal(input))
  })
}
