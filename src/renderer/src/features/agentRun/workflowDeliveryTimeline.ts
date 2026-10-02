import type { ChatAgentTimelineItem, ChatWorkflowDeliveryTimelineItem } from '../chat/chatTypes'

function sequence(item: ChatAgentTimelineItem): number | undefined {
  return item.traceSequence ?? (item.type === 'user_guidance' ? item.sequence : undefined)
}

/** Merge durable mailbox proof without replacing provisional narration or human guidance. */
export function upsertWorkflowDeliveryTimelineItem(
  timeline: ChatAgentTimelineItem[],
  item: ChatWorkflowDeliveryTimelineItem
): ChatAgentTimelineItem[] {
  const remaining = timeline.filter(
    (candidate) => candidate.type !== 'workflow_delivery' || candidate.inputId !== item.inputId
  )
  let previousAnchor = -1
  for (let index = 0; index < remaining.length; index++) {
    const order = sequence(remaining[index])
    if (order !== undefined && order <= item.traceSequence) previousAnchor = index
  }
  const insertionIndex = remaining.findIndex((candidate, index) => {
    const order = sequence(candidate)
    if (order !== undefined) return order > item.traceSequence
    // Recovery may race the next model stream. Its uncommitted text follows the last durable
    // boundary; preserve those deltas, but put the delivered mail before that response.
    return index > previousAnchor && candidate.type === 'message' && Boolean(candidate.streamId)
  })
  if (insertionIndex < 0) return [...remaining, item]
  return [...remaining.slice(0, insertionIndex), item, ...remaining.slice(insertionIndex)]
}
