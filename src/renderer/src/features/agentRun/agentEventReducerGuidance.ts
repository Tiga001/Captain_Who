import type {
  ChatAgentRunView,
  ChatAgentTimelineItem,
  ChatGuidanceTimelineItem,
  ChatMessage,
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
  }>
): ChatGuidanceTimelineItem['attachments'] {
  return attachments.map((attachment) => ({
    id: attachment.id,
    kind: attachment.kind,
    name: attachment.name,
    mimeType: attachment.mimeType,
    sizeBytes: attachment.sizeBytes
  }))
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
          guidanceId: existing.guidanceId ?? item.guidanceId
        }
      : {
          ...existing,
          ...item,
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
  const run = ensureAgentRun(message.agentRun, runId, 'running')
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
