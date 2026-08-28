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
import { stringifyPersistedAgentRun } from '../../features/storage/persistedAgentRun'

const RUN_ID = 'run-sensitive-browser'
const ACTION_ID = '11111111-1111-4111-8111-111111111111'
const APPROVAL_ID = '22222222-2222-4222-8222-222222222222'
const ACTIVATION_ID = '33333333-3333-4333-8333-333333333333'
const CALL_ID = `tc1_${'a'.repeat(43)}`
const HANDLE_CANARY = 'browser-file:PRIVATE_OPAQUE_HANDLE_CANARY'
const BASENAME_CANARY = 'PRIVATE_SELECTED_BASENAME_CANARY.txt'

const action: Extract<AgentProposedAction, { type: 'builtin_mcp_tool_approval' }> = {
  type: 'builtin_mcp_tool_approval',
  approval: {
    schemaVersion: 1,
    identity: {
      actionId: ACTION_ID,
      approvalId: APPROVAL_ID,
      runId: RUN_ID,
      callId: CALL_ID,
      capabilityId: 'browser_automation',
      capabilityActivationId: ACTIVATION_ID,
      managedMcpId: 'builtin.browser_automation.mcp',
      packageName: '@playwright/mcp',
      packageVersion: '0.0.79',
      upstreamCatalogDigest: `sha256:${'1'.repeat(64)}`,
      manifestDigest: `sha256:${'2'.repeat(64)}`,
      policyDigest: `sha256:${'3'.repeat(64)}`,
      policyRevision: 8,
      toolId: 'browser_evaluate',
      rawName: 'browser_evaluate',
      modelName: 'browser_evaluate',
      upstreamSchemaDigest: `sha256:${'4'.repeat(64)}`,
      hostOverlayDigest: `sha256:${'5'.repeat(64)}`,
      hostInputSchemaDigest: `sha256:${'6'.repeat(64)}`,
      argumentsDigest: `sha256:${'7'.repeat(64)}`,
      resourceScopeDigest: `sha256:${'8'.repeat(64)}`,
      origin: 'https://fixture.example'
    },
    capabilityDisplayName: 'Browser automation',
    toolDisplayName: 'Run page script',
    callReason: 'Update the fixture editor.',
    operationCategory: 'page_script_execution',
    resourceSummary: {
      scope: 'page_script_execution',
      displayName: 'Current page script',
      fileBasenames: [],
      origin: 'https://fixture.example'
    },
    riskKinds: ['page_script_execution'],
    createdAt: 1_753_843_200,
    expiresAt: 1_753_844_100,
    approvalStatus: 'required'
  }
}

function baseMessage(): ChatMessage {
  return {
    id: 'assistant-sensitive-browser',
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
      fileChangeProposals: [],
      timeline: []
    }
  }
}

function withOriginalToolCall(): ChatMessage {
  return applyAgentEventToChatMessage(baseMessage(), {
    type: 'tool_call',
    runId: RUN_ID,
    traceSequence: 4,
    identity: {
      type: 'builtin_capability',
      capabilityId: 'browser_automation',
      managedMcpId: 'builtin.browser_automation.mcp',
      packageName: '@playwright/mcp',
      packageVersion: '0.0.79',
      upstreamCatalogDigest: `sha256:${'1'.repeat(64)}`,
      policyDigest: `sha256:${'3'.repeat(64)}`,
      manifestDigest: `sha256:${'2'.repeat(64)}`,
      toolId: 'browser_evaluate',
      rawName: 'browser_evaluate',
      modelName: 'browser_evaluate',
      upstreamSchemaDigest: `sha256:${'4'.repeat(64)}`,
      hostOverlayDigest: `sha256:${'5'.repeat(64)}`,
      hostInputSchemaDigest: `sha256:${'6'.repeat(64)}`
    },
    call: {
      id: CALL_ID,
      tool: 'browser_evaluate',
      args: {},
      approvalStatus: 'not_required',
      reason: 'Update the fixture editor.'
    }
  })
}

function waitingMessage(): ChatMessage {
  return applyAgentEventToChatMessage(withOriginalToolCall(), {
    type: 'approval_required',
    runId: RUN_ID,
    action
  })
}

function execution(status: AgentActionExecutionOutput['status']): AgentActionExecutionOutput {
  return {
    actionId: ACTION_ID,
    actionType: 'builtin_mcp_tool_approval',
    toolName: 'browser_evaluate',
    status,
    agentOutput: {
      status: 'running',
      content: '',
      runId: RUN_ID,
      events: [],
      toolDefinitions: [],
      proposedActions: []
    }
  }
}

describe('built-in MCP Tool approval reducer identity', () => {
  it('hydrates only the exact authoritative pending snapshot after Renderer reload', () => {
    const pending: PendingAgentActionSnapshot = {
      actionId: ACTION_ID,
      actionType: 'builtin_mcp_tool_approval',
      toolName: 'browser_evaluate',
      toolCallId: CALL_ID,
      runId: RUN_ID,
      conversationId: 'conversation-sensitive',
      assistantMessageId: 'assistant-sensitive-browser',
      action,
      createdAt: 1_753_843_200_000,
      status: 'pending'
    }
    expect(shouldHydratePendingAgentAction(pending)).toBe(true)
    expect(shouldHydratePendingAgentAction({ ...pending, status: 'approved' })).toBe(false)
    expect(shouldHydratePendingAgentAction({ ...pending, toolName: 'browser_cookie_get' })).toBe(
      false
    )
    expect(shouldHydratePendingAgentAction({ ...pending, toolCallId: null })).toBe(false)
    expect(
      shouldHydratePendingAgentAction({
        ...pending,
        action: { ...action, approval: { ...action.approval, approvalStatus: 'approved' } }
      })
    ).toBe(false)
  })

  it('keeps the approval inside the original Tool lifecycle and settles it once', () => {
    const waiting = waitingMessage()
    expect(waiting.agentRun?.status).toBe('waiting_for_approval')
    expect(waiting.agentRun?.approvals).toEqual([action])
    expect(waiting.agentRun?.toolCalls).toHaveLength(1)
    expect(waiting.agentRun?.toolCalls[0]?.id).toBe(CALL_ID)

    for (const status of ['approved', 'rejected', 'failed'] as const) {
      const settled = applyAgentActionExecutionToChatMessage(
        waitingMessage(),
        execution(status),
        status === 'rejected' ? 'Do not execute page scripts' : undefined,
        action
      )
      expect(settled.agentRun?.approvals).toEqual([])
      expect(settled.agentRun?.toolCalls).toHaveLength(1)
      expect(settled.agentRun?.toolCalls[0]?.id).toBe(CALL_ID)
      expect(settled.agentRun?.timeline).toEqual([
        expect.objectContaining({ type: 'tool_call', callId: CALL_ID })
      ])
    }
  })

  it('never persists file-selection handles, basenames, raw result bodies or protected approvals', () => {
    const withSelectionResult = applyAgentEventToChatMessage(withOriginalToolCall(), {
      type: 'tool_result',
      runId: RUN_ID,
      result: {
        callId: CALL_ID,
        tool: 'browser_evaluate',
        ok: true,
        result: {
          schemaVersion: 1,
          type: 'builtin_capability_tool',
          status: 'completed',
          contentOmitted: true,
          handle: HANDLE_CANARY,
          basename: BASENAME_CANARY,
          cookie: 'PRIVATE_COOKIE_VALUE_CANARY'
        }
      }
    })
    const encodedResult = JSON.stringify(withSelectionResult.agentRun)
    expect(encodedResult).not.toContain(HANDLE_CANARY)
    expect(encodedResult).not.toContain(BASENAME_CANARY)
    expect(encodedResult).not.toContain('PRIVATE_COOKIE_VALUE_CANARY')

    const persisted = stringifyPersistedAgentRun(waitingMessage().agentRun)
    expect(persisted).not.toBeNull()
    expect(persisted).not.toContain('builtin_mcp_tool_approval')
    expect(persisted).not.toContain(action.approval.identity.argumentsDigest)
  })

  it('drops builtin browser arguments and reasons again at the Renderer persistence boundary', () => {
    const passwordCanary = '使用 UNLABELLED_PASSWORD_CANARY_7Yp9 登录'
    const message = withOriginalToolCall()
    if (!message.agentRun?.toolCalls[0]) throw new Error('Expected browser ToolCall')
    message.agentRun.toolCalls[0] = {
      ...message.agentRun.toolCalls[0],
      args: { call_reason: passwordCanary, text: 'PRIVATE_FORM_VALUE_CANARY' },
      reason: passwordCanary
    }

    const persisted = stringifyPersistedAgentRun(message.agentRun)
    expect(persisted).not.toBeNull()
    expect(persisted).not.toContain(passwordCanary)
    expect(persisted).not.toContain('PRIVATE_FORM_VALUE_CANARY')
    expect(JSON.parse(persisted!).toolCalls[0]).toMatchObject({ args: {}, reason: null })
  })

  it('keeps sensitive rejection feedback process-only at action and persistence boundaries', () => {
    const feedbackCanary = 'REJECTION_FEEDBACK_SECRET_CANARY_Q4z8'
    const rawRejectionResult = {
      callId: CALL_ID,
      tool: 'browser_evaluate',
      ok: true,
      result: {
        schemaVersion: 1,
        type: 'builtin_mcp_tool_approval',
        status: 'rejected',
        decision: 'rejected',
        userFeedback: feedbackCanary,
        retryable: false,
        recovery: 'user_refused_do_not_retry',
        dispatchCertainty: 'definitely_not_dispatched',
        contentOmitted: true
      }
    }
    const rejected = applyAgentActionExecutionToChatMessage(
      waitingMessage(),
      { ...execution('rejected'), toolResult: rawRejectionResult },
      feedbackCanary,
      action
    )
    expect(JSON.stringify(rejected.agentRun)).not.toContain(feedbackCanary)
    expect(rejected.agentRun?.toolResults).toEqual([
      {
        callId: CALL_ID,
        tool: 'browser_evaluate',
        ok: true,
        result: {
          schemaVersion: 1,
          type: 'builtin_capability_tool',
          status: 'rejected',
          contentOmitted: true
        }
      }
    ])

    const compromised = withOriginalToolCall()
    if (!compromised.agentRun) throw new Error('Expected browser Agent run')
    compromised.agentRun.toolResults = [rawRejectionResult]
    const persisted = stringifyPersistedAgentRun(compromised.agentRun)
    expect(persisted).not.toBeNull()
    expect(persisted).not.toContain(feedbackCanary)
    expect(JSON.parse(persisted!).toolResults).toEqual(rejected.agentRun?.toolResults)
  })
})
