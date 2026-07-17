import type {
  WorkspaceDirectoryListing,
  WorkspaceFileMetadata,
  WorkspaceFileRequest,
  WorkspaceImageFileContent,
  WorkspaceListDirectoryInput,
  WorkspaceTextFileContent
} from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'

export function copyWorkspaceFilePath(input: WorkspaceFileRequest): Promise<void> {
  return hostClient.workspaceFiles.copyPath(input)
}

export function listWorkspaceDirectory(
  input: WorkspaceListDirectoryInput
): Promise<WorkspaceDirectoryListing> {
  return hostClient.workspaceFiles.listDirectory(input)
}

export function openWorkspaceExternalLink(url: string): Promise<void> {
  return hostClient.app.openExternal(url)
}

export function readWorkspaceFileMetadata(
  input: WorkspaceFileRequest
): Promise<WorkspaceFileMetadata> {
  return hostClient.workspaceFiles.readFileMetadata(input)
}

export function readWorkspaceTextFile(
  input: WorkspaceFileRequest
): Promise<WorkspaceTextFileContent> {
  return hostClient.workspaceFiles.readTextFile(input)
}

export function readWorkspaceImageFile(
  input: WorkspaceFileRequest
): Promise<WorkspaceImageFileContent> {
  return hostClient.workspaceFiles.readImageFile(input)
}

export function revealWorkspaceFile(input: WorkspaceFileRequest): Promise<void> {
  return hostClient.workspaceFiles.revealInFolder(input)
}
