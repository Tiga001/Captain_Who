import { describe, expect, it } from 'vitest'
import type { AgentEvent } from '@mycopilot/protocol'
import type { ChatMessage, ChatQueuedMessage } from '../../features/chat/chatTypes'
import {
  applyAgentEventToChatMessage,
  applyOptimisticGuidanceToChatMessage
} from '../agentEventReducer'

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
      diffs: [],
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
})
