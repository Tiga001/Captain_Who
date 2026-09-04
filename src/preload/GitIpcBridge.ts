import type { IpcRenderer } from 'electron'
import { HOST_CHANNELS, type GitHostApi } from '@mycopilot/host-api'

type GitIpcRenderer = Pick<IpcRenderer, 'invoke'>

export function createGitIpcBridge(ipcRenderer: GitIpcRenderer): GitHostApi {
  return {
    inspectRepository: (input) => ipcRenderer.invoke(HOST_CHANNELS.git.inspectRepository, input),
    getReviewRepositoryContext: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.git.getReviewRepositoryContext, input),
    listReviewCommits: (input) => ipcRenderer.invoke(HOST_CHANNELS.git.listReviewCommits, input),
    getReviewSummary: (input) => ipcRenderer.invoke(HOST_CHANNELS.git.getReviewSummary, input),
    getTurnDiffSummaries: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.git.getTurnDiffSummaries, input),
    getReviewFileDiff: (input) => ipcRenderer.invoke(HOST_CHANNELS.git.getReviewFileDiff, input),
    getReviewFileContent: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.git.getReviewFileContent, input),
    mutateReviewFile: (input) => ipcRenderer.invoke(HOST_CHANNELS.git.mutateReviewFile, input)
  }
}
