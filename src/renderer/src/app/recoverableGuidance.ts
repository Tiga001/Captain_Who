import type {
  ChatConversation,
  ChatGuidanceTimelineItem,
  ChatMessage
} from '../features/chat/chatTypes'
import { readHumanInteractionGuidanceDisplay } from '../features/humanInteraction/humanInteractionPresentation'

const recoverableGuidanceSignatureCache = new WeakMap<ChatMessage, string>()

export function isRecoverableGuidanceItem(
  item: NonNullable<ChatMessage['agentRun']>['timeline'][number]
): item is ChatGuidanceTimelineItem {
  return (
    item.type === 'user_guidance' &&
    !readHumanInteractionGuidanceDisplay(item) &&
    item.status === 'rejected' &&
    item.recoverable === true &&
    item.rejectionCode === 'run_interrupted'
  )
}

function messageRecoverySignature(message: ChatMessage): string {
  const cached = recoverableGuidanceSignatureCache.get(message)
  if (cached !== undefined) return cached

  const recoverableItems = (message.agentRun?.timeline ?? [])
    .filter(isRecoverableGuidanceItem)
    .map((item) => [
      item.guidanceId ?? null,
      item.clientMessageId,
      item.content,
      item.createdAt,
      item.attachments.map((attachment) => attachment.id),
      (item.folderReferences ?? []).map((folder) => folder.id)
    ])
  const signature = recoverableItems.length > 0 ? JSON.stringify(recoverableItems) : ''
  recoverableGuidanceSignatureCache.set(message, signature)
  return signature
}

/**
 * Returns a stable semantic watermark for the one-shot interrupted-guidance recovery effect.
 * Streaming replaces only the active message, so historical message timelines are cached by
 * identity and do not get rescanned for every visible text delta.
 */
export function getRecoverableGuidanceSignature(
  conversations: readonly ChatConversation[]
): string {
  const conversationsWithRecoveries: Array<
    readonly [string, string | null, string | null, string]
  > = []

  for (const conversation of conversations) {
    if (conversation.messagesLoaded === false) continue
    const messageSignatures = conversation.messages
      .map(messageRecoverySignature)
      .filter(Boolean)
      .join('\u0000')
    if (!messageSignatures) continue
    conversationsWithRecoveries.push([
      conversation.id,
      conversation.modelId,
      conversation.projectId,
      messageSignatures
    ])
  }

  return JSON.stringify(conversationsWithRecoveries)
}
