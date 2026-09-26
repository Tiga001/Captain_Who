import { parseStorageHumanInteractionResponse } from '@mycopilot/protocol'
import type {
  StorageChatConversationMetaRecord,
  StorageChatConversationRecord,
  StorageChatMessageRecord,
  StorageChatMessageWriteRecord,
  StorageChatMessageStateRecord
} from '@mycopilot/protocol'
import type {
  ChatAgentRunView,
  ChatConversation,
  ChatMessage,
  ChatMessageAttachment,
  ChatMessageUiState
} from '../chat/chatTypes'
import { settleAgentRunToolActivities } from '../agentRun/agentEventReducer'
import { parsePersistedAgentRunJson, stringifyPersistedAgentRun } from './persistedAgentRun'
import { parseFolderReferences } from './composerDraftPersistence'

export function mapConversationFromStorage(
  conversation: StorageChatConversationRecord
): ChatConversation {
  return {
    id: conversation.id,
    projectId: conversation.projectId ?? null,
    modelId: conversation.modelId ?? null,
    title: conversation.title,
    messages: conversation.messages.map(mapMessageFromStorage),
    messagesLoaded: true,
    createdAt: conversation.createdAt,
    updatedAt: conversation.updatedAt,
    pinnedAt: conversation.pinnedAt ?? null,
    archivedAt: conversation.archivedAt ?? null,
    unreadAt: conversation.unreadAt ?? null,
    continuationOrigin: conversation.continuationOrigin
      ? {
          sourceConversationId: conversation.continuationOrigin.sourceConversationId,
          sourceMessageId: conversation.continuationOrigin.sourceMessageId,
          boundaryMessageId: conversation.continuationOrigin.boundaryMessageId
        }
      : null
  }
}

export function mapConversationMetaFromStorage(
  conversation: StorageChatConversationMetaRecord
): ChatConversation {
  return {
    id: conversation.id,
    projectId: conversation.projectId ?? null,
    modelId: conversation.modelId ?? null,
    title: conversation.title,
    messages: [],
    messagesLoaded: false,
    createdAt: conversation.createdAt,
    updatedAt: conversation.updatedAt,
    pinnedAt: conversation.pinnedAt ?? null,
    archivedAt: conversation.archivedAt ?? null,
    unreadAt: conversation.unreadAt ?? null
  }
}

export function mapConversationMetaToStorage(
  conversation: ChatConversation
): StorageChatConversationMetaRecord {
  const pendingArchive = conversation.pendingArchivedAt
  return {
    id: conversation.id,
    projectId: conversation.projectId ?? null,
    modelId: conversation.modelId ?? null,
    title: conversation.title,
    createdAt: conversation.createdAt,
    updatedAt: conversation.updatedAt,
    pinnedAt: conversation.pinnedAt ?? null,
    archivedAt: pendingArchive ?? conversation.archivedAt ?? null,
    unreadAt: pendingArchive === undefined ? (conversation.unreadAt ?? null) : null
  }
}

function mapMessageFromStorage(message: StorageChatMessageRecord): ChatMessage {
  const storedRun = parsePersistedAgentRunJson(message.agentRunJson)
  const agentRun =
    storedRun &&
    (storedRun.status === 'completed' ||
      storedRun.status === 'failed' ||
      storedRun.status === 'cancelled')
      ? settleAgentRunToolActivities(
          storedRun,
          storedRun.status,
          storedRun.completedAt ?? message.createdAt
        )
      : storedRun

  return {
    id: message.id,
    role: message.role === 'user' ? 'user' : 'assistant',
    humanInteractionDisplay: parseStorageHumanInteractionResponse(message),
    content: message.content,
    createdAt: message.createdAt,
    status: normalizeMessageStatus(message.status),
    attachments: message.attachments?.map(mapMessageAttachmentFromStorage),
    folderReferences: parseFolderReferences(message.folderReferencesJson),
    agentRun,
    uiState: parseJson<ChatMessageUiState>(message.uiStateJson)
  }
}

export function mapMessageToStorage(message: ChatMessage): StorageChatMessageWriteRecord {
  return {
    id: message.id,
    role: message.role,
    content: message.content,
    createdAt: message.createdAt,
    status: message.status ?? null,
    attachments: [],
    folderReferencesJson: JSON.stringify(message.folderReferences ?? []),
    agentRunJson: stringifyAgentRun(message.agentRun),
    uiStateJson: stringifyJson(message.uiState)
  }
}

export function mapMessageStateToStorage(message: ChatMessage): StorageChatMessageStateRecord {
  return {
    id: message.id,
    content: message.content,
    status: message.status ?? null,
    agentRunJson: stringifyAgentRun(message.agentRun)
  }
}

function mapMessageAttachmentFromStorage(
  attachment: NonNullable<StorageChatMessageRecord['attachments']>[number]
): ChatMessageAttachment {
  return {
    id: attachment.id,
    kind: attachment.kind === 'image' ? 'image' : 'file',
    name: attachment.name,
    mimeType: attachment.mimeType,
    sizeBytes: attachment.sizeBytes,
    previewData: attachment.previewData,
    previewMimeType: attachment.previewMimeType,
    createdAt: attachment.createdAt
  }
}

function parseJson<T>(value: string | null | undefined): T | undefined {
  if (!value) return undefined
  try {
    return JSON.parse(value) as T
  } catch {
    return undefined
  }
}

export function stringifyJson(value: unknown): string | null {
  return value === undefined ? null : JSON.stringify(value)
}

function stringifyAgentRun(run: ChatAgentRunView | undefined): string | null {
  return stringifyPersistedAgentRun(run)
}

function normalizeMessageStatus(status: StorageChatMessageRecord['status']): ChatMessage['status'] {
  if (status === 'pending' || status === 'sent' || status === 'error') return status
  return undefined
}
