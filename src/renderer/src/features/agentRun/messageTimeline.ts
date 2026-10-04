import type { ChatAgentRunView, ChatAgentTimelineItem } from '../chat/chatTypes'

export function removeTransientToolTimelineItems(timeline: ChatAgentTimelineItem[]) {
  return timeline
}

export function upsertMcpInvocationTimelineItem(
  timeline: ChatAgentTimelineItem[],
  invocationId: string,
  callId: string
): ChatAgentTimelineItem[] {
  const id = `mcp-invocation-${invocationId}`
  const marker: ChatAgentTimelineItem = {
    id,
    type: 'mcp_tool_call',
    invocationId
  }
  const isInvocationAnchor = (item: ChatAgentTimelineItem) =>
    (item.type === 'mcp_tool_call' && (item.id === id || item.invocationId === invocationId)) ||
    (item.type === 'tool_call' && item.callId === callId)
  const anchorIndex = timeline.findIndex(isInvocationAnchor)

  if (anchorIndex < 0) {
    return [...timeline, marker]
  }

  return timeline.flatMap((item, index) =>
    isInvocationAnchor(item) ? (index === anchorIndex ? [marker] : []) : [item]
  )
}

export function appendMessageDeltaToTimeline(
  run: ChatAgentRunView,
  delta: string,
  streamId?: string
): ChatAgentTimelineItem[] {
  if (streamId) {
    const itemId = `message-stream-${streamId}`
    const existing = run.timeline.find((item) => item.id === itemId)
    if (existing?.type === 'message') {
      return run.timeline.map((item) =>
        item.id === itemId && item.type === 'message'
          ? { ...item, content: `${item.content}${delta}` }
          : item
      )
    }
    return [...run.timeline, { id: itemId, type: 'message', content: delta, streamId }]
  }
  const lastItem = run.timeline[run.timeline.length - 1]

  if (lastItem?.type === 'message') {
    return run.timeline.map((timelineItem) =>
      timelineItem.id === lastItem.id && timelineItem.type === 'message'
        ? { ...timelineItem, content: `${timelineItem.content}${delta}` }
        : timelineItem
    )
  }

  return [
    ...run.timeline,
    {
      id: `message-${run.timeline.length + 1}`,
      type: 'message',
      content: delta
    }
  ]
}

export function appendMessageToTimeline(
  run: ChatAgentRunView,
  content: string
): ChatAgentTimelineItem[] {
  const lastItem = run.timeline[run.timeline.length - 1]

  if (lastItem?.type === 'message') {
    return run.timeline.map((timelineItem) =>
      timelineItem.id === lastItem.id && timelineItem.type === 'message'
        ? { ...timelineItem, content }
        : timelineItem
    )
  }

  return [
    ...run.timeline,
    {
      id: `message-${run.timeline.length + 1}`,
      type: 'message',
      content
    }
  ]
}

function reconcileCommittedMessages(timeline: ChatAgentTimelineItem[]): ChatAgentTimelineItem[] {
  const positions = new Map<number, number>()
  let duplicated = false
  timeline.forEach((item, index) => {
    if (item.type !== 'message' || item.traceSequence === undefined) return
    const previousIndex = positions.get(item.traceSequence)
    if (previousIndex === undefined) {
      positions.set(item.traceSequence, index)
      return
    }
    duplicated = true
    const previous = timeline[previousIndex]
    // The restored Trace owns the complete body and its ordered position. A stream can still
    // contain only a suffix when history loading races the Renderer delta buffer.
    if (previous.type === 'message' && previous.streamId && !item.streamId) {
      positions.set(item.traceSequence, index)
    }
  })
  if (!duplicated) return timeline
  return timeline.filter(
    (item, index) =>
      item.type !== 'message' ||
      item.traceSequence === undefined ||
      positions.get(item.traceSequence) === index
  )
}

export function commitMessageStreamToTimeline(
  run: ChatAgentRunView,
  streamId: string,
  traceSequence: number | null
): ChatAgentTimelineItem[] {
  if (traceSequence === null) return run.timeline
  return reconcileCommittedMessages(
    run.timeline.map((item) =>
      item.type === 'message' && item.streamId === streamId ? { ...item, traceSequence } : item
    )
  )
}

export function getMessageContentAfterDelta(content: string, delta: string) {
  return `${content}${delta}`
}

export function getFinalMessageContent(currentContent: string, finalContent?: string) {
  return finalContent ?? currentContent
}

export function getFinalTimeline(run: ChatAgentRunView, finalContent?: string) {
  // Both live and restored timelines may already contain presentation copies of one Trace event.
  // Reconcile within this Run by durable identity, never by matching text or display IDs.
  const timeline = reconcileCommittedMessages(removeTransientToolTimelineItems(run.timeline))
  // A completed answer belongs to ChatMessage.content. Its provisional stream has no Trace
  // sequence; committed tool-loop narration does. Use that identity rather than comparing text,
  // since the last persisted stream can lag behind the authoritative final answer.
  if (run.status === 'completed') {
    return timeline.filter(
      (item) => item.type !== 'message' || !item.streamId || item.traceSequence !== undefined
    )
  }
  if (finalContent === undefined) return timeline
  return appendMessageToTimeline({ ...run, timeline }, finalContent)
}

export function getRunResponseTimestamps(run: ChatAgentRunView, receivedAt: number) {
  return {
    firstResponseAt: run.firstResponseAt ?? receivedAt,
    lastResponseAt: receivedAt
  }
}
