import type { ChatAgentRunView, ChatAgentTimelineItem } from '../chat/chatTypes'
import { THINKING_PLACEHOLDER } from './constants'

export function removeTransientToolTimelineItems(timeline: ChatAgentTimelineItem[]) {
  return timeline
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

export function getMessageContentAfterDelta(content: string, delta: string) {
  const previousContent = content === THINKING_PLACEHOLDER ? '' : content
  return `${previousContent}${delta}`
}

export function getFinalMessageContent(currentContent: string, finalContent?: string) {
  if (finalContent === undefined) {
    return currentContent === THINKING_PLACEHOLDER ? '' : currentContent
  }
  return finalContent
}

export function getFinalTimeline(run: ChatAgentRunView, finalContent?: string) {
  const timeline = removeTransientToolTimelineItems(run.timeline)
  if (finalContent === undefined) return timeline
  return appendMessageToTimeline({ ...run, timeline }, finalContent)
}

export function getRunResponseTimestamps(run: ChatAgentRunView, receivedAt: number) {
  return {
    firstResponseAt: run.firstResponseAt ?? receivedAt,
    lastResponseAt: receivedAt
  }
}
