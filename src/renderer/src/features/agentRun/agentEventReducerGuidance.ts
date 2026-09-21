import type {
  ChatAgentRunView,
  ChatAgentTimelineItem,
  ChatGuidanceTimelineItem,
  ChatMessage,
  ChatMessageAttachment,
  ChatQueuedMessage
} from '../chat/chatTypes'
import { appendTimelineItem, ensureAgentRun } from './agentEventReducerShared'

export function guidanceTimelineId(clientMessageId: string) {
  return `user-guidance-${clientMessageId}`
}

export function guidanceAttachments(
  attachments: Array<{
    id: string
    kind: 'file' | 'image'
    name: string
    mimeType?: string
    sizeBytes: number
    encoding?: ChatMessageAttachment['encoding']
    data?: string
    previewData?: string | null
    previewMimeType?: string | null
  }>
): ChatGuidanceTimelineItem['attachments'] {
  return attachments.map((attachment) => ({
    id: attachment.id,
    kind: attachment.kind,
    name: attachment.name,
    mimeType: attachment.mimeType,
    sizeBytes: attachment.sizeBytes,
    ...(attachment.kind === 'image' && attachment.encoding
      ? { encoding: attachment.encoding }
      : {}),
    ...(attachment.kind === 'image' && attachment.data ? { data: attachment.data } : {}),
    ...(attachment.kind === 'image' && attachment.previewData
      ? { previewData: attachment.previewData }
      : {}),
    ...(attachment.kind === 'image' && attachment.previewMimeType
      ? { previewMimeType: attachment.previewMimeType }
      : {})
  }))
}

function mergeGuidanceAttachments(
  attachments: ChatGuidanceTimelineItem['attachments'],
  fallback: ChatGuidanceTimelineItem['attachments']
): ChatGuidanceTimelineItem['attachments'] {
  const fallbackById = new Map(fallback.map((attachment) => [attachment.id, attachment]))
  return attachments.map((attachment) => {
    const previous = fallbackById.get(attachment.id)
    if (!previous) return attachment
    return {
      ...previous,
      ...attachment,
      encoding: attachment.encoding ?? previous.encoding,
      data: attachment.data ?? previous.data,
      previewData: attachment.previewData ?? previous.previewData,
      previewMimeType: attachment.previewMimeType ?? previous.previewMimeType
    }
  })
}

export function upsertGuidanceTimelineItem(
  run: ChatAgentRunView,
  item: ChatGuidanceTimelineItem
): ChatAgentTimelineItem[] {
  const existing = run.timeline.find(
    (candidate) =>
      candidate.type === 'user_guidance' &&
      (candidate.clientMessageId === item.clientMessageId ||
        (item.guidanceId && candidate.guidanceId === item.guidanceId))
  )
  if (!existing || existing.type !== 'user_guidance') {
    if (item.status === 'applied') {
      return insertAppliedGuidanceAtTracePosition(run.timeline, item)
    }
    return appendTimelineItem(run, item)
  }

  const statusRank = { submitting: 0, queued: 1, applied: 2, rejected: 3 } as const
  const nextItem =
    statusRank[existing.status] > statusRank[item.status]
      ? {
          ...item,
          ...existing,
          attachments: mergeGuidanceAttachments(existing.attachments, item.attachments),
          folderReferences: existing.folderReferences ?? item.folderReferences,
          guidanceId: existing.guidanceId ?? item.guidanceId
        }
      : {
          ...existing,
          ...item,
          attachments: mergeGuidanceAttachments(item.attachments, existing.attachments),
          folderReferences: item.folderReferences ?? existing.folderReferences,
          id: existing.id,
          guidanceId: item.guidanceId ?? existing.guidanceId
        }
  if (item.status !== 'applied' || existing.status === 'applied') {
    return run.timeline.map((candidate) => (candidate.id === existing.id ? nextItem : candidate))
  }

  return insertAppliedGuidanceAtTracePosition(
    run.timeline.filter((candidate) => candidate.id !== existing.id),
    nextItem
  )
}

function insertAppliedGuidanceAtTracePosition(
  timeline: ChatAgentTimelineItem[],
  item: ChatGuidanceTimelineItem
): ChatAgentTimelineItem[] {
  const appliedSequence = item.traceSequence ?? item.sequence
  if (appliedSequence === undefined) return [...timeline, item]

  const insertionIndex = timeline.findIndex((candidate) => {
    const candidateSequence =
      candidate.traceSequence ??
      (candidate.type === 'user_guidance' ? candidate.sequence : undefined)
    return candidateSequence !== undefined && candidateSequence > appliedSequence
  })

  if (insertionIndex < 0) return [...timeline, item]
  return [...timeline.slice(0, insertionIndex), item, ...timeline.slice(insertionIndex)]
}

export function applyOptimisticGuidanceToChatMessage(
  message: ChatMessage,
  queuedMessage: ChatQueuedMessage,
  runId: string
): ChatMessage {
  // Accepting guidance does not resume an approval pause; only the Host's decision does.
  const run = ensureAgentRun(message.agentRun, runId, message.agentRun?.status ?? 'running')
  return {
    ...message,
    status: 'pending',
    agentRun: {
      ...run,
      timeline: upsertGuidanceTimelineItem(run, {
        id: guidanceTimelineId(queuedMessage.clientMessageId),
        type: 'user_guidance',
        clientMessageId: queuedMessage.clientMessageId,
        content: queuedMessage.content,
        attachments: guidanceAttachments(queuedMessage.attachments),
        folderReferences: queuedMessage.folderReferences,
        status: 'submitting',
        createdAt: queuedMessage.createdAt
      })
    }
  }
}

export function removeGuidanceFromChatMessage(
  message: ChatMessage,
  clientMessageId: string
): ChatMessage {
  if (!message.agentRun) return message
  return {
    ...message,
    agentRun: {
      ...message.agentRun,
      timeline: message.agentRun.timeline.filter(
        (item) => item.type !== 'user_guidance' || item.clientMessageId !== clientMessageId
      )
    }
  }
}
