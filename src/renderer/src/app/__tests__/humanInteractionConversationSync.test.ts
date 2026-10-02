import { expect, it, vi } from 'vitest'
import type {
  ChatConversation,
  ChatMessage,
  ChatWorkflowDeliveryTimelineItem
} from '../../features/chat/chatTypes'
import { ensureAgentRun } from '../../features/agentRun/agentEventReducer'
vi.mock('../../host/hostClient', () => ({ hostClient: {} }))
import { isRecoverableGuidanceItem } from '../recoverableGuidance'
import { humanInteractionResponseDisplay } from '../../features/humanInteraction/humanInteractionState'
import {
  question,
  submitted
} from '../../features/humanInteraction/__tests__/humanInteractionFixtures'
import { mergeHumanInteractionConversation } from '../useHumanInteractionConversationSync'
import { applyWorkflowDeliveryToConversation } from '../workflowConversationDelivery'

const baseline: ChatConversation = {
  id: 'chat',
  title: 'Original',
  modelId: null,
  projectId: null,
  createdAt: 1,
  updatedAt: 1,
  messages: [{ id: 'old', role: 'assistant', content: 'Before', createdAt: 1 }]
}
it('attaches Host-created answer/assistant rows and preserves streaming and pending local messages received during a load', () => {
  const current: ChatConversation = {
    ...baseline,
    title: 'Renamed',
    messages: [
      { ...baseline.messages[0], content: 'New live text', uiState: { favorited: true } },
      { id: 'local', role: 'user', content: 'Composer input', status: 'pending', createdAt: 4 }
    ]
  }
  const stored: ChatConversation = {
    ...baseline,
    updatedAt: 3,
    messages: [
      ...baseline.messages,
      { id: 'answer', role: 'user', content: 'Host answer', createdAt: 2 },
      { id: 'new-run', role: 'assistant', content: 'Continued', createdAt: 3 }
    ]
  }
  const next = mergeHumanInteractionConversation(current, stored, baseline)
  expect(next.messages.map((message) => message.id)).toEqual(['old', 'answer', 'new-run', 'local'])
  expect(next.messages[0]).toBe(current.messages[0])
  expect(next.title).toBe('Renamed')
  expect(current.messages).toHaveLength(2)
})
it('refreshes unchanged waiting history from Host without changing metadata or local UI state', () => {
  const stored = {
    ...baseline,
    messages: [{ ...baseline.messages[0], content: 'Authoritative result' }]
  }
  expect(mergeHumanInteractionConversation(baseline, stored, baseline).messages[0].content).toBe(
    'Authoritative result'
  )
})

it('keeps Host-owned answered guidance out of Composer recovery without swallowing ordinary JSON input', () => {
  const item = {
    id: 'answer',
    type: 'user_guidance' as const,
    guidanceId: 'g1',
    clientMessageId: 'human-answer-g1',
    content: JSON.stringify(humanInteractionResponseDisplay(submitted(question()))),
    attachments: [],
    createdAt: 1,
    status: 'rejected' as const,
    recoverable: true,
    rejectionCode: 'run_interrupted'
  }
  expect(isRecoverableGuidanceItem(item)).toBe(false)
  expect(isRecoverableGuidanceItem({ ...item, clientMessageId: 'ordinary-guidance-id' })).toBe(true)
})

const delivery = (inputId = 'mail', traceSequence = 4): ChatWorkflowDeliveryTimelineItem => ({
  id: `workflow-delivery-${inputId}`,
  type: 'workflow_delivery',
  inputId,
  deliveryId: `delivery-${inputId}`,
  instanceId: 'workflow',
  workflowName: 'Review',
  content: `Wrapped ${inputId}`,
  createdAt: 2,
  traceSequence,
  sources: [
    {
      nodeId: 'boss',
      nodeName: 'Boss',
      conversationId: 'boss-chat',
      conversationTitle: 'Boss chat',
      content: `Body ${inputId}`
    }
  ]
})
const mailBubble = (inputId = 'mail'): ChatMessage => ({
  id: `delivery-${inputId}`,
  role: 'user',
  content: `Wrapped ${inputId}`,
  createdAt: 2,
  workflowSource: {
    inputId,
    instanceId: 'workflow',
    workflowName: 'Review',
    sources: delivery(inputId).sources
  }
})
function runningConversation(): ChatConversation {
  return {
    ...baseline,
    messages: [
      mailBubble('startup'),
      {
        id: 'assistant',
        role: 'assistant',
        content: 'Live narration',
        createdAt: 3,
        status: 'pending',
        agentRun: {
          ...ensureAgentRun(undefined, 'run', 'running'),
          timeline: [
            { id: 'before', type: 'message', content: 'Before accept', traceSequence: 1 },
            { id: 'accept', type: 'tool_call', callId: 'accept', traceSequence: 2 },
            { id: 'stream', type: 'message', content: 'Live narration', streamId: 'stream' }
          ]
        }
      }
    ]
  }
}

it.each([false, true])(
  'merges applied mail into a live turn without losing stream text (stream changed during read: %s)',
  (changedDuringRead) => {
    const before = runningConversation()
    const current = changedDuringRead
      ? {
          ...before,
          messages: before.messages.map((message) =>
            message.role === 'assistant' ? { ...message, content: 'New live text' } : message
          )
        }
      : before
    const stored = runningConversation()
    stored.messages[1] = {
      ...stored.messages[1],
      content: 'Stale stored text',
      agentRun: {
        ...stored.messages[1].agentRun!,
        timeline: [...stored.messages[1].agentRun!.timeline.slice(0, 2), delivery()]
      }
    }
    const next = mergeHumanInteractionConversation(current, stored, before)
    expect(next.messages[1].content).toBe(changedDuringRead ? 'New live text' : 'Live narration')
    expect(next.messages[1].agentRun!.timeline.map((item) => item.id)).toEqual([
      'before',
      'accept',
      'workflow-delivery-mail',
      'stream'
    ])
    expect(current.messages[1].agentRun!.timeline).toHaveLength(3)
  }
)

it('removes only proven detached mail copies and retains startup mail, pending guidance and local messages', () => {
  const before = runningConversation()
  const current: ChatConversation = {
    ...before,
    messages: [
      ...before.messages,
      mailBubble(),
      mailBubble('claimed'),
      { id: 'local', role: 'user', content: 'Unsaved input', status: 'pending', createdAt: 8 }
    ]
  }
  const stored = runningConversation()
  stored.messages[1].agentRun!.timeline = [
    ...stored.messages[1].agentRun!.timeline.slice(0, 2),
    delivery()
  ]
  const next = mergeHumanInteractionConversation(current, stored, before)
  expect(next.messages.map((message) => message.id)).toEqual([
    'delivery-startup',
    'assistant',
    'delivery-claimed',
    'local'
  ])
  const repeated = mergeHumanInteractionConversation(next, stored, next)
  expect(repeated.messages.map((message) => message.id)).toEqual(next.messages.map((m) => m.id))
  expect(
    repeated.messages[1].agentRun!.timeline.filter((item) => item.type === 'workflow_delivery')
  ).toHaveLength(1)
})

it('keeps accepted deliveries when a terminal snapshot lags the last live delivery', () => {
  const current = runningConversation()
  const run = current.messages[1].agentRun!
  run.timeline = [run.timeline[0], run.timeline[1], delivery(), delivery('later', 7)]
  const stored = runningConversation()
  stored.messages[1].agentRun = {
    ...stored.messages[1].agentRun!,
    status: 'completed',
    timeline: [delivery(), { id: 'final', type: 'message', content: 'Done', traceSequence: 9 }]
  }
  const next = mergeHumanInteractionConversation(current, stored, current)
  expect(next.messages[1].agentRun!.status).toBe('completed')
  expect(next.messages[1].agentRun!.timeline.map((item) => item.id)).toEqual([
    'workflow-delivery-mail',
    'workflow-delivery-later',
    'final'
  ])
})

it('does not merge a delivery into a different run or remove an unrelated message with similar content', () => {
  const current = runningConversation()
  current.messages.push({ ...mailBubble(), id: 'unrelated' })
  const stored = runningConversation()
  stored.messages[1].agentRun = {
    ...stored.messages[1].agentRun!,
    runId: 'other-run',
    timeline: [delivery()]
  }
  const next = mergeHumanInteractionConversation(current, stored, baseline)
  expect(next.messages[1].agentRun?.runId).toBe('run')
  expect(
    next.messages[1].agentRun!.timeline.some((item) => item.type === 'workflow_delivery')
  ).toBe(false)
  expect(next.messages.at(-1)?.id).toBe('unrelated')
})

it.each(['completed', 'cancelled'] as const)(
  'routes a late durable delivery to its owner without reviving a %s turn',
  (status) => {
    const current = runningConversation()
    current.messages[1].agentRun!.status = status
    current.messages[1].status = 'sent'
    current.messages.push(mailBubble())
    const mail = delivery()
    const event = {
      ...mail,
      type: 'workflow_delivery_applied' as const,
      conversationId: 'chat',
      assistantMessageId: 'assistant',
      runId: 'run',
      sequence: mail.traceSequence
    }
    const next = applyWorkflowDeliveryToConversation(current, event)
    expect(next.messages[1].agentRun!.status).toBe(status)
    expect(next.messages[1].status).toBe('sent')
    expect(next.messages.map((message) => message.id)).toEqual(['delivery-startup', 'assistant'])
    expect(next.messages[1].agentRun!.timeline.map((item) => item.id)).toEqual([
      'before',
      'accept',
      'workflow-delivery-mail',
      'stream'
    ])
    expect(applyWorkflowDeliveryToConversation(current, { ...event, runId: 'wrong' })).toBe(current)
    expect(
      applyWorkflowDeliveryToConversation(current, { ...event, conversationId: 'wrong' })
    ).toBe(current)
  }
)
