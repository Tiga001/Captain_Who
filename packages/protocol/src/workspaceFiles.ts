export type WorkspaceDirectoryEntryKind = 'directory' | 'file' | 'symlink'

export interface WorkspaceDirectoryEntry {
  kind: WorkspaceDirectoryEntryKind
  name: string
  path: string
}

export interface WorkspaceListDirectoryInput {
  directoryPath?: string
  folderId?: string
  includeHidden?: boolean
  projectId: string
}

export interface WorkspaceDirectoryListing {
  directoryPath: string
  entries: WorkspaceDirectoryEntry[]
  truncated: boolean
}

/** A safe, display-only result for the composer @ workspace search. */
export type WorkspaceMentionEntryKind = 'directory' | 'file'

export interface WorkspaceMentionSearchInput {
  projectId: string
  query: string
  limit?: number
}

export interface WorkspaceMentionSearchEntry {
  alias: string
  displayName: string
  folderId: string
  kind: WorkspaceMentionEntryKind
  path: string
  /** Relative path including the folder root, suitable for display only. */
  displayPath: string
}

export interface WorkspaceMentionSearchResult {
  entries: WorkspaceMentionSearchEntry[]
  truncated: boolean
}

export type WorkspaceFilePreviewKind =
  'binary' | 'image' | 'pdf' | 'text' | 'too-large' | 'unsupported'

export interface WorkspaceFileRequest {
  /** Historical files use the originating Turn's frozen folder binding. */
  assistantMessageId?: string
  folderId?: string
  path: string
  projectId: string
}

/** A persisted message attachment. Native paths are resolved only by the Host. */
export interface AttachmentFileRequest {
  attachmentId: string
  messageId: string
}

/** A file actually referenced by a persisted or Host-observed read call in this Turn. */
export interface ReadToolFileRequest {
  source: 'read-tool'
  conversationId: string
  assistantMessageId: string
  callId: string
  filePath: string
  projectId?: string | null
}

export type WorkspaceFilePreviewRequest =
  WorkspaceFileRequest | AttachmentFileRequest | ReadToolFileRequest

export interface WorkspaceFileMetadata {
  /** Original attachment name, which may differ from the stored filename. */
  name?: string
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

export interface WorkspacePdfFileContent {
  data: Uint8Array
  mimeType: 'application/pdf'
  modifiedAtMs: number
  path: string
  sizeBytes: number
}

export interface WorkspaceFilePreviewResult {
  image?: WorkspaceImageFileContent
  metadata: WorkspaceFileMetadata
  pdf?: WorkspacePdfFileContent
  text?: WorkspaceTextFileContent
}
