import type {
  AgentEvent,
  AgentMcpToolApproval,
  AgentMcpToolInvocationEvent
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import {
  applyAgentActionDecisionToChatMessage,
  applyAgentActionExecutionToChatMessage,
  applyAgentEventToChatMessage,
  shouldTouchConversationForAgentEvent
} from '../../features/agentRun/agentEventReducer'
import type { ChatAgentRunView, ChatMessage } from '../../features/chat/chatTypes'

const RUN_ID = 'run-mcp-activity'
const ACTION_ID = '11111111-1111-4111-8111-111111111111'
const INVOCATION_ID = '22222222-2222-4222-8222-222222222222'
const SERVER_ID = '33333333-3333-4333-8333-333333333333'
const CALL_ID = `tc1_${'a'.repeat(43)}`
const SECRET_CANARY = 'MCP_RAW_ARGUMENT_CANARY_DO_NOT_STORE'

function message(): ChatMessage {
  const agentRun: ChatAgentRunView = {
    runId: RUN_ID,
    status: 'running',
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    diffs: [],
    mcpInvocations: [],
    timeline: []
  }
  return {
    id: 'assistant-mcp-activity',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun
  }
}

function approval(): AgentMcpToolApproval {
  return {
    identity: {
      actionId: ACTION_ID,
      invocationId: INVOCATION_ID,
      runId: RUN_ID,
      callId: CALL_ID,
      provenance: {
        serverId: SERVER_ID,
        scope: { type: 'user' },
        rawToolName: 'echo_text',
        modelToolName: 'model-visible-name-without-an-mcp-prefix',
        configEpoch: '44444444-4444-4444-8444-444444444444',
        registryRevision: 7,
        configDigest: 'a'.repeat(64),
        catalogGeneration: 2,
        catalogDigest: 'b'.repeat(64),
        catalogSchemaDigest: 'c'.repeat(64),
        schemaDigest: 'd'.repeat(64),
        schemaNormalizerVersion: 1
      }
    },
    call: {
      id: CALL_ID,
      tool: 'model-visible-name-without-an-mcp-prefix',
      args: {},
      approvalStatus: 'required'
    },
    summary: {
      serverId: SERVER_ID,
      serverDisplayName: 'Owned fixture',
      scope: { type: 'user' },
      rawToolName: 'echo_text',
      modelToolName: 'model-visible-name-without-an-mcp-prefix',
      arguments: {
        encodedBytes: 24,
        topLevelPropertyCount: 1,
        stringValueCount: 1,
        numberValueCount: 0,
        booleanValueCount: 0,
        nullValueCount: 0,
        objectValueCount: 1,
        arrayValueCount: 0,
        maxDepth: 2,
        truncated: false
      },
      risk: 'side_effects_possible',
      external: true
    },
    approvalMode: 'prompt',
    payloadPersistence: 'process_only',
    createdAt: 100,
    expiresAt: 1000
  }
}

function lifecycle(
  state: AgentMcpToolInvocationEvent['state'],
  overrides: Partial<AgentMcpToolInvocationEvent> = {}
): Extract<AgentEvent, { type: 'mcp_tool_invocation_state_changed' }> {
  const terminal =
    state === 'completed'
      ? {
          dispatchCertainty: 'response_received' as const,
          outcome: 'succeeded' as const,
          isError: false,
          durationMs: 25
        }
      : state === 'outcome_unknown'
        ? {
            dispatchCertainty: 'possibly_dispatched' as const,
            outcome: 'outcome_unknown' as const,
            errorCode: 'response_lost'
          }
        : state === 'running' || state === 'dispatching'
          ? { dispatchCertainty: 'possibly_dispatched' as const }
          : { dispatchCertainty: 'definitely_not_dispatched' as const }

  return {
    type: 'mcp_tool_invocation_state_changed',
    runId: RUN_ID,
    invocation: {
      actionId: ACTION_ID,
      invocationId: INVOCATION_ID,
      callId: CALL_ID,
      serverId: SERVER_ID,
      serverDisplayName: 'Owned fixture',
      rawToolName: 'echo_text',
      modelToolName: 'model-visible-name-without-an-mcp-prefix',
      external: true,
      state,
      outputTruncated: false,
      ...terminal,
      ...overrides
    }
  }
}

describe('MCP lifecycle Renderer projection', () => {
  it('uses typed approval identity without retaining the redacted AgentToolCall', () => {
    const projected = applyAgentEventToChatMessage(message(), {
      type: 'approval_required',
      runId: RUN_ID,
      action: { type: 'mcp_tool_call', approval: approval() }
    })

    expect(projected.agentRun?.toolCalls).toEqual([])
    expect(projected.agentRun?.toolResults).toEqual([])
    expect(projected.agentRun?.mcpInvocations).toEqual([
      {
        actionId: ACTION_ID,
        invocationId: INVOCATION_ID,
        callId: CALL_ID,
        serverId: SERVER_ID,
        serverDisplayName: 'Owned fixture',
        scope: { type: 'user' },
        rawToolName: 'echo_text',
        modelToolName: 'model-visible-name-without-an-mcp-prefix',
        external: true,
        state: 'pending_approval',
        dispatchCertainty: 'definitely_not_dispatched',
        outputTruncated: false
      }
    ])
    expect(projected.agentRun?.timeline).toEqual([
      {
        id: `mcp-invocation-${INVOCATION_ID}`,
        type: 'mcp_tool_call',
        invocationId: INVOCATION_ID
      }
    ])
  })

  it('projects lifecycle fields explicitly and does not retain future wire fields', () => {
    const event = lifecycle('running')
    Object.assign(event.invocation, {
      rawArguments: SECRET_CANARY,
      rawResult: SECRET_CANARY,
      structuredContent: SECRET_CANARY,
      stderr: SECRET_CANARY,
      payloadRef: SECRET_CANARY
    })

    const projected = applyAgentEventToChatMessage(message(), event)
    const serialized = JSON.stringify(projected.agentRun?.mcpInvocations)

    expect(serialized).not.toContain(SECRET_CANARY)
    expect(projected.agentRun?.mcpInvocations?.[0]).toEqual({
      actionId: ACTION_ID,
      invocationId: INVOCATION_ID,
      callId: CALL_ID,
      serverId: SERVER_ID,
      serverDisplayName: 'Owned fixture',
      rawToolName: 'echo_text',
      modelToolName: 'model-visible-name-without-an-mcp-prefix',
      external: true,
      state: 'running',
      dispatchCertainty: 'possibly_dispatched',
      outputTruncated: false
    })
  })

  it('deduplicates replay and never lets an out-of-order or later terminal event regress state', () => {
    const running = applyAgentEventToChatMessage(message(), lifecycle('running'))
    const replayed = applyAgentEventToChatMessage(running, lifecycle('running'))
    const stale = applyAgentEventToChatMessage(replayed, lifecycle('approved'))
    const completed = applyAgentEventToChatMessage(stale, lifecycle('completed'))
    const laterUnknown = applyAgentEventToChatMessage(completed, lifecycle('outcome_unknown'))

    expect(laterUnknown.agentRun?.mcpInvocations).toHaveLength(1)
    expect(laterUnknown.agentRun?.mcpInvocations?.[0].state).toBe('completed')
    expect(
      laterUnknown.agentRun?.timeline.filter((item) => item.type === 'mcp_tool_call')
    ).toHaveLength(1)
  })

  it('enriches a lifecycle-first invocation with approval scope without regressing state', () => {
    const running = applyAgentEventToChatMessage(message(), lifecycle('running'))
    const withApproval = applyAgentEventToChatMessage(running, {
      type: 'approval_required',
      runId: RUN_ID,
      action: { type: 'mcp_tool_call', approval: approval() }
    })

    expect(withApproval.agentRun?.mcpInvocations?.[0]).toMatchObject({
      state: 'running',
      scope: { type: 'user' }
    })
  })

  it('treats completed isError as a completed Tool-level error', () => {
    const projected = applyAgentEventToChatMessage(
      message(),
      lifecycle('completed', {
        outcome: 'tool_error',
        isError: true,
        errorCode: 'server_tool_error'
      })
    )

    expect(projected.agentRun?.mcpInvocations?.[0]).toMatchObject({
      state: 'completed',
      outcome: 'tool_error',
      isError: true,
      dispatchCertainty: 'response_received'
    })
  })

  it('drops generic MCP call and result bodies once typed callId provenance is known', () => {
    const approved = applyAgentEventToChatMessage(message(), {
      type: 'approval_required',
      runId: RUN_ID,
      action: { type: 'mcp_tool_call', approval: approval() }
    })
    const withCall = applyAgentEventToChatMessage(approved, {
      type: 'tool_call',
      runId: RUN_ID,
      call: {
        id: CALL_ID,
        tool: 'model-visible-name-without-an-mcp-prefix',
        args: { value: SECRET_CANARY },
        approvalStatus: 'approved'
      }
    })
    const withResult = applyAgentEventToChatMessage(withCall, {
      type: 'tool_result',
      runId: RUN_ID,
      result: {
        callId: CALL_ID,
        tool: 'model-visible-name-without-an-mcp-prefix',
        ok: true,
        result: { value: SECRET_CANARY }
      }
    })

    expect(withResult.agentRun?.toolCalls).toEqual([])
    expect(withResult.agentRun?.toolResults).toEqual([])
    expect(JSON.stringify(withResult.agentRun)).not.toContain(SECRET_CANARY)
  })

  it('does not create or retain generic Tool state when an MCP approval is rejected or cancelled', () => {
    const action = { type: 'mcp_tool_call' as const, approval: approval() }
    const waiting = applyAgentEventToChatMessage(message(), {
      type: 'approval_required',
      runId: RUN_ID,
      action
    })
    const contaminated: ChatMessage = {
      ...waiting,
      agentRun: waiting.agentRun
        ? {
            ...waiting.agentRun,
            toolCalls: [
              {
                id: CALL_ID,
                tool: 'provider-safe-name',
                args: { value: SECRET_CANARY },
                approvalStatus: 'required'
              }
            ],
            toolResults: [
              {
                callId: CALL_ID,
                tool: 'provider-safe-name',
                ok: true,
                result: { value: SECRET_CANARY }
              }
            ],
            timeline: [
              ...waiting.agentRun.timeline,
              { id: `tool-call-${CALL_ID}`, type: 'tool_call', callId: CALL_ID }
            ]
          }
        : undefined
    }

    const rejected = applyAgentActionDecisionToChatMessage(
      contaminated,
      action,
      'rejected',
      SECRET_CANARY
    )

    expect(rejected.agentRun?.toolCalls).toEqual([])
    expect(rejected.agentRun?.toolResults).toEqual([])
    expect(rejected.agentRun?.timeline.some((item) => item.type === 'tool_call')).toBe(false)
    expect(JSON.stringify(rejected.agentRun)).not.toContain(SECRET_CANARY)
  })

  it('ignores a generic ToolResult body on an MCP action execution output', () => {
    const action = { type: 'mcp_tool_call' as const, approval: approval() }
    const waiting = applyAgentEventToChatMessage(message(), {
      type: 'approval_required',
      runId: RUN_ID,
      action
    })
    const executed = applyAgentActionExecutionToChatMessage(waiting, {
      actionId: ACTION_ID,
      actionType: 'mcp_tool_call',
      toolName: 'provider-safe-name',
      status: 'approved',
      toolResult: {
        callId: CALL_ID,
        tool: 'provider-safe-name',
        ok: true,
        result: { rawResult: SECRET_CANARY }
      },
      agentOutput: {
        status: 'running',
        content: '',
        runId: RUN_ID,
        events: [],
        toolDefinitions: [],
        proposedActions: []
      }
    })

    expect(executed.agentRun?.toolResults).toEqual([])
    expect(JSON.stringify(executed.agentRun)).not.toContain(SECRET_CANARY)
  })

  it('does not infer MCP identity from an mcp__ model name', () => {
    const generic = applyAgentEventToChatMessage(message(), {
      type: 'tool_call',
      runId: RUN_ID,
      call: {
        id: 'ordinary-tool-call',
        tool: 'mcp__spoofed__name',
        args: { visibleGenericArgument: true },
        approvalStatus: 'not_required'
      }
    })

    expect(generic.agentRun?.toolCalls).toHaveLength(1)
    expect(generic.agentRun?.timeline).toContainEqual({
      id: 'tool-call-ordinary-tool-call',
      type: 'tool_call',
      callId: 'ordinary-tool-call'
    })
    expect(generic.agentRun?.mcpInvocations).toEqual([])
  })

  it('marks lifecycle events as durable conversation changes', () => {
    expect(shouldTouchConversationForAgentEvent(lifecycle('running'))).toBe(true)
  })
})
