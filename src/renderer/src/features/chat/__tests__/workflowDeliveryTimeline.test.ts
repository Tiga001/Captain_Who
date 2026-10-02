import { describe, expect, it } from 'vitest'
import type { AgentEvent } from '@mycopilot/protocol'
import type {
  ChatAgentTimelineItem,
  ChatMessage,
  ChatWorkflowDeliveryTimelineItem
} from '../chatTypes'
import { applyAgentEventToChatMessage } from '../../agentRun/agentEventReducer'
import { upsertWorkflowDeliveryTimelineItem } from '../../agentRun/workflowDeliveryTimeline'
import { parseTimelineItem } from '../../storage/persistedAgentRunTimelineValidators'

const mail: ChatWorkflowDeliveryTimelineItem = {
  id: 'workflow-delivery-input-1',
  type: 'workflow_delivery',
  inputId: 'input-1',
  deliveryId: 'workflow-message-input-1',
  instanceId: 'workflow-1',
  workflowName: '文案润色',
  content: 'Complete assembled organization context',
  createdAt: 1,
  traceSequence: 4,
  sources: [
    {
      nodeId: 'writer',
      nodeName: '作者',
      conversationId: 'chat-1',
      conversationTitle: '文书',
      content: '稿件正文'
    }
  ]
}
const applied: Extract<AgentEvent, { type: 'workflow_delivery_applied' }> = {
  type: 'workflow_delivery_applied',
  conversationId: 'conversation-1',
  runId: 'run-1',
  assistantMessageId: 'assistant-1',
  inputId: mail.inputId,
  deliveryId: mail.deliveryId,
  instanceId: mail.instanceId,
  workflowName: mail.workflowName,
  content: mail.content,
  createdAt: mail.createdAt,
  sequence: mail.traceSequence,
  sources: mail.sources
}
function message(timeline: ChatAgentTimelineItem[] = []): ChatMessage {
  return {
    id: 'assistant-1',
    role: 'assistant',
    content: '最新的回复',
    createdAt: 10,
    status: 'pending',
    agentRun: {
      runId: 'run-1',
      status: 'running',
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline
    }
  }
}

describe('organization delivery timeline', () => {
  it('inserts durable deliveries by trace order, not mailbox creation time, and deduplicates replay', () => {
    const original = message([
      { id: 'before', type: 'message', content: 'Before', traceSequence: 1 },
      { id: 'accept', type: 'tool_call', callId: 'accept', traceSequence: 2 },
      { id: 'after', type: 'message', content: 'After', traceSequence: 5 }
    ])
    const once = applyAgentEventToChatMessage(original, applied)
    const twice = applyAgentEventToChatMessage(once, applied)
    expect(twice.agentRun?.timeline.map((item) => item.id)).toEqual([
      'before',
      'accept',
      mail.id,
      'after'
    ])
    expect(twice.agentRun?.timeline[2]).toEqual(mail)
    expect(twice.content).toBe(original.content)
  })

  it('keeps provisional model deltas while inserting recovered mail before the next response', () => {
    const pendingGuidance: ChatAgentTimelineItem = {
      id: 'guidance',
      type: 'user_guidance',
      clientMessageId: 'human-1',
      content: 'Pending human guidance',
      attachments: [],
      status: 'queued',
      createdAt: 20
    }
    const stream: ChatAgentTimelineItem = {
      id: 'stream',
      type: 'message',
      content: 'Still streaming',
      streamId: 'stream-1'
    }
    const result = upsertWorkflowDeliveryTimelineItem(
      [
        { id: 'before', type: 'message', content: 'Before', traceSequence: 1 },
        { id: 'accept', type: 'tool_call', callId: 'accept', traceSequence: 2 },
        pendingGuidance,
        stream
      ],
      mail
    )
    expect(result.map((item) => item.id)).toEqual([
      'before',
      'accept',
      'guidance',
      mail.id,
      'stream'
    ])
    expect(result[2]).toBe(pendingGuidance)
    expect(result[4]).toBe(stream)
  })

  it('rejects another run or assistant and fills a settled run without resuming it', () => {
    const original = message()
    expect(applyAgentEventToChatMessage(original, { ...applied, runId: 'other' })).toBe(original)
    expect(
      applyAgentEventToChatMessage(original, { ...applied, assistantMessageId: 'other' })
    ).toBe(original)
    const unbound = { ...original, agentRun: undefined }
    expect(applyAgentEventToChatMessage(unbound, applied)).toBe(unbound)
    expect(applyAgentEventToChatMessage(original, { ...applied, runId: 'other-run' })).toBe(
      original
    )
    const completed = {
      ...original,
      status: 'sent' as const,
      agentRun: { ...original.agentRun!, status: 'completed' as const, completedAt: 50 }
    }
    const result = applyAgentEventToChatMessage(completed, applied)
    expect(result.status).toBe('sent')
    expect(result.agentRun?.status).toBe('completed')
    expect(result.agentRun?.completedAt).toBe(50)
    expect(result.agentRun?.timeline).toEqual([mail])
  })

  it('restores the exact persisted delivery and fails closed on missing provenance', () => {
    expect(parseTimelineItem(JSON.parse(JSON.stringify(mail)))).toEqual(mail)
    expect(parseTimelineItem({ ...mail, sources: [] })).toBeUndefined()
    expect(parseTimelineItem({ ...mail, deliveryId: undefined })).toBeUndefined()
    expect(parseTimelineItem({ ...mail, traceSequence: -1 })).toBeUndefined()
    expect(
      parseTimelineItem({ ...mail, sources: [{ ...mail.sources[0], content: undefined }] })
    ).toBeUndefined()
    expect(parseTimelineItem({ ...mail, approval: true })).toBeUndefined()
  })
})
