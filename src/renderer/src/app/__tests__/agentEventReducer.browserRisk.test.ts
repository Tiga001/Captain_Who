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

const RUN_ID = 'run-browser-risk'
const ACTION_ID = '11111111-1111-4111-8111-111111111111'
const RISK_ID = '22222222-2222-4222-8222-222222222222'
const ACTIVATION_ID = '33333333-3333-4333-8333-333333333333'
const CALL_ID = `tc1_${'a'.repeat(43)}`

const action: Extract<AgentProposedAction, { type: 'browser_risk_approval' }> = {
  type: 'browser_risk_approval',
  approval: {
    schemaVersion: 1,
    actionId: ACTION_ID,
    riskApprovalId: RISK_ID,
    runId: RUN_ID,
    callId: CALL_ID,
    capabilityId: 'browser_automation',
    capabilityActivationId: ACTIVATION_ID,
    displayName: 'Browser automation',
    reason: 'Open the local test dashboard.',
    destination: {
      normalizedUrl: 'http://127.0.0.1:3000/dashboard',
      origin: 'http://127.0.0.1:3000',
      scheme: 'http',
      asciiHost: '127.0.0.1',
      effectivePort: 3000,
      addressClass: 'loopback'
    },
    trigger: 'tool_argument',
    triggerToolName: 'browser_navigate',
    riskKinds: ['insecure_http', 'loopback', 'non_standard_port'],
    manifestDigest: `sha256:${'b'.repeat(64)}`,
    policyRevision: 7,
    createdAt: 1_753_843_200,
    expiresAt: 1_753_844_100,
    approvalStatus: 'required'
  }
}

function baseMessage(): ChatMessage {
  return {
    id: 'assistant-browser-risk',
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

function withBrowserCall(): ChatMessage {
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
      policyDigest: `sha256:${'2'.repeat(64)}`,
      manifestDigest: `sha256:${'b'.repeat(64)}`,
      toolId: 'browser.navigate',
      rawName: 'browser_navigate',
      modelName: 'browser_navigate',
      upstreamSchemaDigest: `sha256:${'3'.repeat(64)}`,
      hostOverlayDigest: `sha256:${'4'.repeat(64)}`,
      hostInputSchemaDigest: `sha256:${'5'.repeat(64)}`
    },
    call: {
      id: CALL_ID,
      tool: 'browser_navigate',
      args: {},
      approvalStatus: 'not_required',
      reason: 'Open the local test dashboard.'
    }
  })
}

function waitingMessage(): ChatMessage {
  return applyAgentEventToChatMessage(withBrowserCall(), {
    type: 'approval_required',
    runId: RUN_ID,
    action
  })
}

function execution(status: AgentActionExecutionOutput['status']): AgentActionExecutionOutput {
  return {
    actionId: ACTION_ID,
    actionType: 'browser_risk_approval',
    toolName: 'browser_navigate',
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

describe('browser risk approval reducer identity', () => {
  it('hydrates only the exact process-owned pending snapshot', () => {
    const pending: PendingAgentActionSnapshot = {
      actionId: ACTION_ID,
      actionType: 'browser_risk_approval',
      toolName: 'browser_navigate',
      toolCallId: CALL_ID,
      runId: RUN_ID,
      conversationId: 'conversation-browser-risk',
      assistantMessageId: 'assistant-browser-risk',
      action,
      createdAt: 1_753_843_200_000,
      status: 'pending'
    }
    expect(shouldHydratePendingAgentAction(pending)).toBe(true)
    expect(shouldHydratePendingAgentAction({ ...pending, status: 'approved' })).toBe(false)
    expect(shouldHydratePendingAgentAction({ ...pending, toolName: 'browser_click' })).toBe(false)
    expect(shouldHydratePendingAgentAction({ ...pending, toolCallId: null })).toBe(false)
    expect(
      shouldHydratePendingAgentAction({
        ...pending,
        action: { ...action, approval: { ...action.approval, approvalStatus: 'approved' } }
      })
    ).toBe(false)
  })

  it('does not create a second generic Tool activity for the risk decision', () => {
    const waiting = waitingMessage()
    expect(waiting.agentRun?.status).toBe('waiting_for_approval')
    expect(waiting.agentRun?.approvals).toEqual([action])
    expect(waiting.agentRun?.toolCalls).toHaveLength(1)
    expect(waiting.agentRun?.toolCalls[0]?.id).toBe(CALL_ID)
    expect(waiting.agentRun?.timeline).toEqual([
      expect.objectContaining({ type: 'tool_call', callId: CALL_ID })
    ])
  })

  it.each(['approved', 'rejected', 'failed'] as const)(
    'settles %s without deleting or duplicating the original browser call',
    (status) => {
      const settled = applyAgentActionExecutionToChatMessage(
        waitingMessage(),
        execution(status),
        status === 'rejected' ? 'Use HTTPS instead' : undefined,
        action
      )
      expect(settled.agentRun?.approvals).toEqual([])
      expect(settled.agentRun?.toolCalls).toHaveLength(1)
      expect(settled.agentRun?.toolCalls[0]?.id).toBe(CALL_ID)
      expect(settled.agentRun?.timeline).toEqual([
        expect.objectContaining({ type: 'tool_call', callId: CALL_ID })
      ])
    }
  )

  it('stores only an allowlisted result receipt for managed browser Tools', () => {
    const withResult = applyAgentEventToChatMessage(withBrowserCall(), {
      type: 'tool_result',
      runId: RUN_ID,
      result: {
        callId: CALL_ID,
        tool: 'browser_navigate',
        ok: false,
        result: { accessibilityTree: 'PRIVATE_PAGE_TREE_CANARY' },
        error: 'AUTHORIZATION_COOKIE_CANARY'
      }
    })
    expect(withResult.agentRun?.toolResults).toEqual([
      { callId: CALL_ID, tool: 'browser_navigate', ok: false }
    ])
    expect(JSON.stringify(withResult.agentRun)).not.toContain('PRIVATE_PAGE_TREE_CANARY')
    expect(JSON.stringify(withResult.agentRun)).not.toContain('AUTHORIZATION_COOKIE_CANARY')
  })

  it('drops non-allowlisted fields while retaining the rebuilt managed receipt', () => {
    const withResult = applyAgentEventToChatMessage(withBrowserCall(), {
      type: 'tool_result',
      runId: RUN_ID,
      result: {
        callId: CALL_ID,
        tool: 'browser_navigate',
        ok: false,
        result: {
          schemaVersion: 1,
          type: 'builtin_capability_tool',
          status: 'outcome_unknown',
          contentOmitted: true,
          privateDetail: 'PRIVATE_PAGE_TREE_CANARY'
        },
        error: 'AUTHORIZATION_COOKIE_CANARY'
      }
    })
    expect(withResult.agentRun?.toolResults).toEqual([
      {
        callId: CALL_ID,
        tool: 'browser_navigate',
        ok: false,
        result: {
          schemaVersion: 1,
          type: 'builtin_capability_tool',
          status: 'outcome_unknown',
          contentOmitted: true
        }
      }
    ])
    expect(JSON.stringify(withResult.agentRun)).not.toContain('PRIVATE_PAGE_TREE_CANARY')
    expect(JSON.stringify(withResult.agentRun)).not.toContain('AUTHORIZATION_COOKIE_CANARY')
  })

  it('preserves a strictly parsed Artifact reference and cancelled receipt', () => {
    const artifact = {
      schemaVersion: 1,
      artifactId: 'browser-artifact:123e4567-e89b-42d3-a456-426614174000',
      kind: 'snapshot',
      displayName: 'page.txt',
      mimeType: 'text/plain',
      sizeBytes: 4,
      createdAt: 1_000,
      expiresAt: 2_000,
      lifecycle: 'run',
      owner: 'browser_automation',
      preview: 'text'
    } as const
    const withResult = applyAgentEventToChatMessage(withBrowserCall(), {
      type: 'tool_result',
      runId: RUN_ID,
      result: {
        callId: CALL_ID,
        tool: 'browser_snapshot',
        ok: false,
        result: {
          schemaVersion: 1,
          type: 'builtin_capability_tool',
          status: 'cancelled',
          contentOmitted: true,
          artifacts: [artifact]
        }
      }
    })

    expect(withResult.agentRun?.toolResults).toEqual([
      {
        callId: CALL_ID,
        tool: 'browser_snapshot',
        ok: false,
        result: {
          schemaVersion: 1,
          type: 'builtin_capability_tool',
          status: 'cancelled',
          contentOmitted: true,
          artifacts: [artifact]
        }
      }
    ])
  })
})
