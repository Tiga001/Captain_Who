import type { AgentToolIdentity } from '@mycopilot/protocol'
import type { ChatAgentRunView, ChatAgentTimelineItem } from '../chat/chatTypes'
import { removeTransientToolTimelineItems } from './messageTimeline'
import { appendTimelineItem } from './agentEventReducerShared'

export function appendToolCallToTimeline(
  run: ChatAgentRunView,
  callId: string,
  traceSequence?: number,
  identity?: AgentToolIdentity
): ChatAgentTimelineItem[] {
  const existing = run.timeline.find((item) => item.type === 'tool_call' && item.callId === callId)
  const stableTraceSequence = traceSequence ?? existing?.traceSequence
  const stableIdentity =
    identity ?? (existing?.type === 'tool_call' ? existing.identity : undefined)
  return appendTimelineItem(
    {
      ...run,
      timeline: removeTransientToolTimelineItems(run.timeline)
    },
    {
      id: `tool-call-${callId}`,
      type: 'tool_call',
      callId,
      ...(stableIdentity === undefined ? {} : { identity: stableIdentity }),
      ...(stableTraceSequence === undefined ? {} : { traceSequence: stableTraceSequence })
    }
  )
}

function contextCompactionTimelineId(operationId: string) {
  return `context-compaction-${operationId}`
}

export function startContextCompaction(
  run: ChatAgentRunView,
  operationId: string,
  traceSequence: number
): ChatAgentTimelineItem[] {
  const id = contextCompactionTimelineId(operationId)
  if (run.timeline.some((item) => item.id === id)) return run.timeline

  return appendTimelineItem(run, {
    id,
    type: 'context_compaction',
    operationId,
    status: 'running',
    traceSequence
  })
}

export function finishContextCompaction(
  run: ChatAgentRunView,
  operationId: string,
  status: Extract<ChatAgentTimelineItem, { type: 'context_compaction' }>['status'],
  traceSequence: number
): ChatAgentTimelineItem[] {
  return appendTimelineItem(run, {
    id: contextCompactionTimelineId(operationId),
    type: 'context_compaction',
    operationId,
    status,
    traceSequence
  })
}

export function settlePendingContextCompactions(
  timeline: ChatAgentTimelineItem[],
  status: 'completed' | 'failed' | 'cancelled'
): ChatAgentTimelineItem[] {
  const compactionStatus =
    status === 'completed' ? 'applied' : status === 'cancelled' ? 'cancelled' : 'failed'

  return timeline.map((item) =>
    item.type === 'context_compaction' && item.status === 'running'
      ? { ...item, status: compactionStatus }
      : item
  )
}
