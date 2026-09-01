import { describe, expect, it } from 'vitest'
import type { AgentEvent } from '@mycopilot/protocol'
import type { ChatMessage, ChatQueuedMessage } from '../../features/chat/chatTypes'
import {
  applyAgentActionExecutionToChatMessage,
  applyAgentEventToChatMessage,
  applyOptimisticGuidanceToChatMessage
} from '../../features/agentRun/agentEventReducer'

function assistantMessage(): ChatMessage {
  return {
    id: 'assistant-1',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun: {
      runId: 'run-1',
      status: 'running',
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline: []
    }
  }
}

function queuedMessage(): ChatQueuedMessage {
  return {
    id: 'queue-1',
    clientMessageId: 'client-1',
    content: 'Inspect this\n\nAttachments: notes.txt',
    attachments: [
      {
        id: 'attachment-1',
        kind: 'file',
        name: 'notes.txt',
        mimeType: 'text/plain',
        sizeBytes: 5,
        encoding: 'base64',
        data: 'aGVsbG8='
      }
    ],
    modelId: 'model-1',
    permissionMode: 'default',
    projectId: null,
    skills: [],
    status: 'pending',
    createdAt: 10
  }
}

function guidanceEvent(
  type: 'guidance_queued' | 'guidance_applied'
): Extract<AgentEvent, { type: 'guidance_queued' | 'guidance_applied' }> {
  const common = {
    runId: 'run-1',
    guidanceId: 'guidance-1',
    clientMessageId: 'client-1',
    content: 'Inspect this\n\nAttachments: notes.txt',
    attachments: [
      {
        id: 'attachment-1',
        kind: 'file' as const,
        name: 'notes.txt',
        mimeType: 'text/plain',
        sizeBytes: 5
      }
    ],
    createdAt: 10
  }
  return type === 'guidance_applied' ? { type, ...common, sequence: 4 } : { type, ...common }
}

describe('agent guidance timeline projection', () => {
  it('reconciles optimistic, queued and applied updates into one metadata-only item', () => {
    const optimistic = applyOptimisticGuidanceToChatMessage(
      assistantMessage(),
      queuedMessage(),
      'run-1'
    )
    const queued = applyAgentEventToChatMessage(optimistic, guidanceEvent('guidance_queued'))
    const applied = applyAgentEventToChatMessage(queued, guidanceEvent('guidance_applied'))
    const replayedQueued = applyAgentEventToChatMessage(applied, guidanceEvent('guidance_queued'))

    expect(replayedQueued.agentRun?.timeline).toEqual([
      expect.objectContaining({
        type: 'user_guidance',
        clientMessageId: 'client-1',
        guidanceId: 'guidance-1',
        status: 'applied',
        sequence: 4,
        traceSequence: 4,
        attachments: [
          {
            id: 'attachment-1',
            kind: 'file',
            name: 'notes.txt',
            mimeType: 'text/plain',
            sizeBytes: 5
          }
        ]
      })
    ])
    expect(JSON.stringify(replayedQueued.agentRun?.timeline)).not.toContain('aGVsbG8=')
  })

  it('moves guidance from its optimistic send position to its applied Trace position', () => {
    const messageBeforeGuidance: ChatMessage = {
      ...assistantMessage(),
      agentRun: {
        ...assistantMessage().agentRun!,
        timeline: [
          {
            id: 'message-before',
            type: 'message',
            content: 'Before the user sent guidance.',
            traceSequence: 0
          }
        ]
      }
    }
    const optimistic = applyOptimisticGuidanceToChatMessage(
      messageBeforeGuidance,
      queuedMessage(),
      'run-1'
    )
    if (!optimistic.agentRun) throw new Error('missing optimistic Agent Run')
    const withLaterWork: ChatMessage = {
      ...optimistic,
      agentRun: {
        ...optimistic.agentRun,
        timeline: [
          ...optimistic.agentRun.timeline,
          {
            id: 'tool-after-send',
            type: 'tool_call',
            callId: 'call-1',
            traceSequence: 1
          },
          {
            id: 'message-after-apply',
            type: 'message',
            content: 'After the guidance took effect.',
            traceSequence: 5
          }
        ]
      }
    }

    const applied = applyAgentEventToChatMessage(withLaterWork, guidanceEvent('guidance_applied'))

    expect(applied.agentRun?.timeline.map((item) => item.id)).toEqual([
      'message-before',
      'tool-after-send',
      'user-guidance-client-1',
      'message-after-apply'
    ])
    expect(applied.agentRun?.timeline.filter((item) => item.type === 'user_guidance')).toHaveLength(
      1
    )

    const replayed = applyAgentEventToChatMessage(applied, guidanceEvent('guidance_applied'))
    expect(replayed.agentRun?.timeline.map((item) => item.id)).toEqual(
      applied.agentRun?.timeline.map((item) => item.id)
    )
  })

  it('removes rejected optimistic guidance from the timeline', () => {
    const optimistic = applyOptimisticGuidanceToChatMessage(
      assistantMessage(),
      queuedMessage(),
      'run-1'
    )
    const rejected = applyAgentEventToChatMessage(optimistic, {
      type: 'guidance_rejected',
      runId: 'run-1',
      guidanceId: 'guidance-1',
      clientMessageId: 'client-1',
      content: 'Inspect this',
      rejectionCode: 'run_not_steerable',
      message: 'Run ended.',
      createdAt: 10
    })

    expect(rejected.agentRun?.timeline).toEqual([])
  })

  it('keeps guidance disabled until an approved continuation actually starts', () => {
    const waiting: ChatMessage = {
      ...assistantMessage(),
      agentRun: {
        ...assistantMessage().agentRun!,
        status: 'waiting_for_approval'
      }
    }
    const accepted = applyAgentActionExecutionToChatMessage(waiting, {
      actionId: 'action-1',
      actionType: 'tool_call',
      toolName: 'apply_patch',
      status: 'applied',
      agentOutput: {
        content: '',
        status: 'running',
        runId: 'run-1',
        events: [],
        toolDefinitions: [],
        proposedActions: []
      }
    })

    expect(accepted.agentRun?.status).toBe('starting')

    const started = applyAgentEventToChatMessage(accepted, {
      type: 'state',
      runId: 'run-1',
      state: {
        status: 'running',
        activeRunId: 'run-1',
        lastError: null,
        updatedAt: 20
      }
    })
    expect(started.agentRun?.status).toBe('running')
  })
})
