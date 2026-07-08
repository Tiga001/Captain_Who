// Protocol layer.
import type { AgentInputAttachment } from './agent'

export type AttachmentSelectionKind = 'file' | 'image'

export interface AttachmentSelectInputRequest {
  kind: AttachmentSelectionKind
}

export interface AttachmentLoadFromPathsRequest {
  paths: string[]
}

export type AttachmentInputPayload = AgentInputAttachment
