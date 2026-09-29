import type {
  WorkspaceDirectoryListing,
  WorkspaceFilePreviewResult,
  WorkspaceFilePreviewRequest,
  WorkspaceListDirectoryInput,
  WorkspaceMentionSearchInput,
  WorkspaceMentionSearchResult
} from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'

export function copyWorkspaceFilePath(input: WorkspaceFilePreviewRequest): Promise<void> {
  return hostClient.workspaceFiles.copyPath(input)
}

export function listWorkspaceDirectory(
  input: WorkspaceListDirectoryInput
): Promise<WorkspaceDirectoryListing> {
  return hostClient.workspaceFiles.listDirectory(input)
}

export function searchWorkspaceMentions(
  input: WorkspaceMentionSearchInput
): Promise<WorkspaceMentionSearchResult> {
  return hostClient.workspaceFiles.searchMentions(input)
}

export function openWorkspaceExternalLink(url: string): Promise<void> {
  return hostClient.app.openExternal(url)
}

export function readWorkspaceFilePreview(
  input: WorkspaceFilePreviewRequest
): Promise<WorkspaceFilePreviewResult> {
  return hostClient.workspaceFiles.readPreview(input)
}

export function revealWorkspaceFile(input: WorkspaceFilePreviewRequest): Promise<void> {
  return hostClient.workspaceFiles.revealInFolder(input)
}
