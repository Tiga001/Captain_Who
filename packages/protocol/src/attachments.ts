import type { AgentInputAttachment } from './agent'

export type AttachmentSelectionKind = 'file' | 'image'

/** Durable Host-only identity used to detect a replaced folder after restart. */
export type AgentFolderIdentity =
  | {
      kind: 'unix'
      schemaVersion: number
      device: number
      inode: number
    }
  | {
      kind: 'windows'
      schemaVersion: number
      volumeSerialNumber: number
      fileId: string
    }

/**
 * A user-selected directory reference.  Unlike a file attachment this does not
 * contain directory bytes; the Host owns the path grant and the agent resolves
 * files from it on demand.
 */
export interface AgentFolderReference {
  schemaVersion: number
  id: string
  name: string
  /** Selected absolute folder path, visible to the model for ordinary tool calls. */
  rootPath?: string
  /** Host-only durable identity; omitted from model-facing projections. */
  rootIdentity?: AgentFolderIdentity
  /** Host-only availability marker from persisted metadata. */
  status?: 'available' | 'unavailable'
}

export interface FolderSelectInputRequest {
  requestId?: string
}

export interface FolderLoadFromPathsRequest {
  paths: string[]
}

export interface AttachmentSelectInputRequest {
  kind: AttachmentSelectionKind
  requestId?: string
}

export interface AttachmentLoadFromPathsRequest {
  paths: string[]
}

export type AttachmentInputPayload = AgentInputAttachment

export type AttachmentImportMetadata = Pick<
  AgentInputAttachment,
  'id' | 'kind' | 'name' | 'mimeType' | 'sizeBytes'
>

export interface AttachmentImportHandle {
  importId: string
}

export interface AttachmentImportChunk extends AttachmentImportHandle {
  offset: number
  data: string
}

export interface AttachmentImportProgress {
  requestId?: string
  attachment: AttachmentImportMetadata
  receivedBytes: number
  status: 'importing' | 'complete' | 'failed' | 'cancelled'
  error?: string
}

export interface AttachmentPreview {
  mimeType: string
  data: string
}
