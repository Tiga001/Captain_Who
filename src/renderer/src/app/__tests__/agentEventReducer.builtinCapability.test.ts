import type {
  AgentActionExecutionOutput,
  AgentProposedAction,
  PendingAgentActionSnapshot
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import {
  applyAgentActionExecutionToChatMessage,
  applyAgentEventToChatMessage
} from '../../features/agentRun/agentEventReducer'
import { shouldHydratePendingAgentAction } from '../../features/agentRun/agentActionUtils'
import type { ChatMessage } from '../../features/chat/chatTypes'

const RUN_ID = 'run-builtin-capability'
const ACTION_ID = '11111111-1111-4111-8111-111111111111'
const ACTIVATION_ID = '22222222-2222-4222-8222-222222222222'
const CALL_ID = `tc1_${'a'.repeat(43)}`

const action: Extract<AgentProposedAction, { type: 'builtin_capability_activation' }> = {
  type: 'builtin_capability_activation',
  approval: {
    actionId: ACTION_ID,
    activationId: ACTIVATION_ID,
    runId: RUN_ID,
    callId: CALL_ID,
    capabilityId: 'browser_automation',
    displayName: 'Browser automation',
    reason: 'Open the in-app browser for this task.',
    manifestDigest: `sha256:${'b'.repeat(64)}`,
    policyRevision: 7,
    createdAt: 1_753_843_200,
    expiresAt: 1_753_844_100,
    approvalStatus: 'required'
  }
}

function baseMessage(): ChatMessage {
  return {
    id: 'assistant-builtin-capability',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun: {
      runId: RUN_ID,
      status: 'running',
      startedAt: 1,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      diffs: [],
      timeline: []
    }
  }
}

function waitingMessage(): ChatMessage {
  return applyAgentEventToChatMessage(baseMessage(), {
    type: 'approval_required',
    runId: RUN_ID,
    action
  })
}

function execution(status: AgentActionExecutionOutput['status']): AgentActionExecutionOutput {
  return {
    actionId: ACTION_ID,
    actionType: 'builtin_capability_activation',
    toolName: 'activate_capability',
    status,
    agentOutput: {
      status: status === 'failed' ? 'failed' : 'running',
      content: '',
      runId: RUN_ID,
      events: [],
      toolDefinitions: [],
      proposedActions: []
    }
  }
}

function expectActivationProjectionRemoved(message: ChatMessage) {
  expect(message.agentRun?.approvals).toEqual([])
  expect(message.agentRun?.toolCalls).toEqual([])
  expect(message.agentRun?.toolResults).toEqual([])
  expect(message.agentRun?.timeline.some((item) => item.type === 'tool_call')).toBe(false)
}

describe('built-in capability activation reducer identity', () => {
  it('hydrates only an authoritative pending/required activation snapshot', () => {
    const pending: PendingAgentActionSnapshot = {
      actionId: ACTION_ID,
      actionType: 'builtin_capability_activation',
      toolName: 'activate_capability',
      toolCallId: CALL_ID,
      runId: RUN_ID,
      conversationId: 'conversation-builtin-capability',
      assistantMessageId: 'assistant-builtin-capability',
      action,
      createdAt: 1_753_843_200_000,
      status: 'pending'
    }

    expect(shouldHydratePendingAgentAction(pending)).toBe(true)
    for (const status of [
      'approved',
      'executing',
      'rejected',
      'cancelled',
      'completed',
      'failed'
    ] as const) {
      expect(shouldHydratePendingAgentAction({ ...pending, status })).toBe(false)
    }
    expect(
      shouldHydratePendingAgentAction({
        ...pending,
        action: {
          ...action,
          approval: { ...action.approval, approvalStatus: 'approved' }
        }
      })
    ).toBe(false)
    expect(shouldHydratePendingAgentAction({ ...pending, toolCallId: null })).toBe(false)
  })

  it('anchors the approval by model callId rather than Host actionId', () => {
    const waiting = waitingMessage()

    expect(waiting.agentRun?.approvals).toEqual([action])
    expect(waiting.agentRun?.toolCalls).toEqual([
      expect.objectContaining({ id: CALL_ID, tool: 'activate_capability' })
    ])
    expect(waiting.agentRun?.timeline).toEqual([
      { id: `tool-call-${CALL_ID}`, type: 'tool_call', callId: CALL_ID }
    ])
  })

  it('keeps the Host-authored built-in Tool identity on the timeline item', () => {
    const identity = {
      type: 'builtin_capability' as const,
      capabilityId: 'browser_automation' as const,
      managedMcpId: 'builtin.browser_automation.mcp',
      packageName: '@playwright/mcp',
      packageVersion: '0.0.79',
      upstreamCatalogDigest: `sha256:${'1'.repeat(64)}`,
      policyDigest: `sha256:${'2'.repeat(64)}`,
      manifestDigest: `sha256:${'c'.repeat(64)}`,
      toolId: 'browser.navigate',
      rawName: 'browser_navigate',
      modelName: 'browser_navigate',
      upstreamSchemaDigest: `sha256:${'3'.repeat(64)}`,
      hostOverlayDigest: `sha256:${'4'.repeat(64)}`,
      hostInputSchemaDigest: `sha256:${'5'.repeat(64)}`
    }
    const next = applyAgentEventToChatMessage(baseMessage(), {
      type: 'tool_call',
      runId: RUN_ID,
      traceSequence: 4,
      identity,
      call: {
        id: `tc1_${'d'.repeat(43)}`,
        tool: 'browser_navigate',
        args: {},
        approvalStatus: 'approved',
        reason: null
      }
    })

    expect(next.agentRun?.timeline).toEqual([
      expect.objectContaining({ type: 'tool_call', identity })
    ])
  })

  it.each(['approved', 'rejected', 'failed'] as const)(
    'removes the synthetic activation projection after a %s terminal action response',
    (status) => {
      const settled = applyAgentActionExecutionToChatMessage(
        waitingMessage(),
        execution(status),
        undefined,
        action
      )

      expectActivationProjectionRemoved(settled)
    }
  )

  it('settles a late action response without reopening a completed run', () => {
    const completed = applyAgentEventToChatMessage(waitingMessage(), {
      type: 'done',
      runId: RUN_ID,
      success: true,
      status: 'completed',
      content: 'Browser capability is ready.',
      proposedActions: []
    })
    expect(completed.agentRun?.approvals).toEqual([])
    expect(completed.agentRun?.toolCalls).toHaveLength(1)

    const settled = applyAgentActionExecutionToChatMessage(
      completed,
      execution('approved'),
      undefined,
      action
    )

    expect(settled.content).toBe('Browser capability is ready.')
    expect(settled.status).toBe('sent')
    expect(settled.agentRun?.status).toBe('completed')
    expectActivationProjectionRemoved(settled)
  })
})
