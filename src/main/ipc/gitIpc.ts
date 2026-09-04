import { HOST_CHANNELS } from '@mycopilot/host-api'
import type { CoreServer } from '../core/coreServer'
import type { TrustedIpcMain } from './trustedIpc'

export function registerGitIpc(ipcMain: TrustedIpcMain, coreServer: CoreServer): void {
  ipcMain.handle(HOST_CHANNELS.git.inspectRepository, (_event, input) =>
    coreServer.inspectGitRepository(input)
  )
  ipcMain.handle(HOST_CHANNELS.git.getReviewRepositoryContext, (_event, input) =>
    coreServer.getGitReviewRepositoryContext(input)
  )
  ipcMain.handle(HOST_CHANNELS.git.listReviewCommits, (_event, input) =>
    coreServer.listGitReviewCommits(input)
  )
  ipcMain.handle(HOST_CHANNELS.git.getReviewSummary, (_event, input) =>
    coreServer.getGitReviewSummary(input)
  )
  ipcMain.handle(HOST_CHANNELS.git.getTurnDiffSummaries, (_event, input) =>
    coreServer.getGitTurnDiffSummaries(input)
  )
  ipcMain.handle(HOST_CHANNELS.git.getReviewFileDiff, (_event, input) =>
    coreServer.getGitReviewFileDiff(input)
  )
  ipcMain.handle(HOST_CHANNELS.git.getReviewFileContent, (_event, input) =>
    coreServer.getGitReviewFileContent(input)
  )
  ipcMain.handle(HOST_CHANNELS.git.mutateReviewFile, (_event, input) =>
    coreServer.mutateGitReviewFile(input)
  )
}
