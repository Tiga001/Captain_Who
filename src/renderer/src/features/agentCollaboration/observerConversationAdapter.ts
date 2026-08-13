import type { AgentObserverConversation } from '@mycopilot/protocol'
import type { ChatConversation, ChatMessage, ChatMessageUiState } from '../chat/chatTypes'
import { settleAgentRunToolActivities } from '../agentRun/agentEventReducer'
import { parsePersistedAgentRunJson } from '../storage/persistedAgentRun'

function parseObserverUiState(value: string | null): ChatMessageUiState | undefined {
  if (!value) return undefined
  let parsed: unknown
  try {
    parsed = JSON.parse(value) as unknown
  } catch {
    return undefined
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return undefined
  const record = parsed as Record<string, unknown>
  if (
    Object.keys(record).some((key) => key !== 'favorited' && key !== 'timelineCollapsed') ||
    (record.favorited !== undefined && typeof record.favorited !== 'boolean') ||
    (record.timelineCollapsed !== undefined && typeof record.timelineCollapsed !== 'boolean')
  ) {
    return undefined
  }
  return {
    ...(record.favorited === true ? { favorited: true } : {}),
    ...(typeof record.timelineCollapsed === 'boolean'
      ? { timelineCollapsed: record.timelineCollapsed }
      : {})
  }
}

function mapObserverMessage(message: AgentObserverConversation['messages'][number]): ChatMessage {
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
    id: message.messageId,
    role: message.role === 'user' ? 'user' : 'assistant',
    content: message.content,
    createdAt: message.createdAt,
    status:
      message.status === 'pending' || message.status === 'sent' || message.status === 'error'
        ? message.status
        : undefined,
    attachments: message.attachments.map((attachment) => {
      const isAuthorizedImageBytes =
        attachment.kind === 'image' &&
        Boolean(attachment.previewData) &&
        Boolean(attachment.previewMimeType?.startsWith('image/'))
      return {
        id: attachment.attachmentId,
        kind: attachment.kind === 'image' ? ('image' as const) : ('file' as const),
        name: attachment.name,
        mimeType: attachment.mimeType,
        sizeBytes: attachment.sizeBytes,
        previewData: attachment.previewData,
        previewMimeType: attachment.previewMimeType,
        createdAt: attachment.createdAt,
        ...(isAuthorizedImageBytes
          ? {
              encoding: 'base64' as const,
              data: attachment.previewData ?? undefined,
              mimeType: attachment.previewMimeType
            }
          : {})
      }
    }),
    agentRun,
    uiState: parseObserverUiState(message.uiStateJson),
    ...(message.inputOrigin ? { inputOrigin: message.inputOrigin } : {})
  }
}

/**
 * Converts the authorized observer DTO into the one renderer chat projection. This is a display
 * adapter only: child conversations never enter ordinary history, drafts, or persistence queues.
 */
export function mapObserverConversationToChat(
  observer: AgentObserverConversation
): ChatConversation {
  return {
    id: observer.conversationId,
    projectId: observer.projectId,
    modelId: observer.modelId,
    title: observer.title,
    messages: observer.messages.map(mapObserverMessage),
    messagesLoaded: true,
    createdAt: observer.createdAt,
    updatedAt: observer.updatedAt,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null,
    continuationOrigin: null
  }
}
