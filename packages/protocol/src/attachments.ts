import type { AgentInputAttachment } from './agent'

export type AttachmentSelectionKind = 'file' | 'image'

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
