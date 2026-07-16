export type WorkspaceDirectoryEntryKind = 'directory' | 'file' | 'symlink'

export interface WorkspaceDirectoryEntry {
  kind: WorkspaceDirectoryEntryKind
  name: string
  path: string
}

export interface WorkspaceListDirectoryInput {
  directoryPath?: string
  includeHidden?: boolean
  projectId: string
}

export interface WorkspaceDirectoryListing {
  directoryPath: string
  entries: WorkspaceDirectoryEntry[]
  truncated: boolean
}

export type WorkspaceFilePreviewKind = 'binary' | 'image' | 'text' | 'too-large' | 'unsupported'

export interface WorkspaceFileRequest {
  path: string
  projectId: string
}

export interface WorkspaceFileMetadata {
  kind: WorkspaceDirectoryEntryKind
  mimeType: string | null
  modifiedAtMs: number
  path: string
  previewKind: WorkspaceFilePreviewKind
  sizeBytes: number
}

export interface WorkspaceTextFileContent {
  content: string
  modifiedAtMs: number
  path: string
  sizeBytes: number
}

export interface WorkspaceImageFileContent {
  data: string
  mimeType: string
  modifiedAtMs: number
  path: string
  sizeBytes: number
}
