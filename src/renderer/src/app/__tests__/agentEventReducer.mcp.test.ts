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
    fileChangeProposals: [],
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
      approvalStatus: 'required',
      reason: null
    },
    summary: {
      serverId: SERVER_ID,
      serverDisplayName: 'Owned fixture',
      scope: { type: 'user' },
      rawToolName: 'echo_text',
      modelToolName: 'model-visible-name-without-an-mcp-prefix',
      displayReason: 'Read the fixture inventory',
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
  const terminal: Partial<AgentMcpToolInvocationEvent> =
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
      ...overrides,
      dispatchCertainty:
        overrides.dispatchCertainty ?? terminal.dispatchCertainty ?? 'definitely_not_dispatched',
      displayReason: overrides.displayReason ?? null,
      outcome: overrides.outcome ?? terminal.outcome ?? null,
      isError: overrides.isError ?? terminal.isError ?? null,
      errorCode: overrides.errorCode ?? terminal.errorCode ?? null,
      durationMs: overrides.durationMs ?? terminal.durationMs ?? null,
      diagnostics: overrides.diagnostics ?? null
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
        displayReason: 'Read the fixture inventory',
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

  it('converts a generic Tool timeline placeholder in place without disturbing messages', () => {
    const before = applyAgentEventToChatMessage(message(), {
      type: 'message_delta',
      runId: RUN_ID,
      streamId: 'before-mcp',
      delta: 'Before MCP'
    })
    const generic = applyAgentEventToChatMessage(before, {
      type: 'tool_call',
      runId: RUN_ID,
      traceSequence: 1,
      identity: { type: 'mcp', provenance: approval().identity.provenance },
      call: {
        id: CALL_ID,
        tool: 'model-visible-name-without-an-mcp-prefix',
        args: {},
        approvalStatus: 'approved',
        reason: null
      }
    })
    const surrounded = applyAgentEventToChatMessage(generic, {
      type: 'message_delta',
      runId: RUN_ID,
      streamId: 'after-mcp',
      delta: 'After MCP'
    })
    const genericIndex =
      surrounded.agentRun?.timeline.findIndex(
        (item) => item.type === 'tool_call' && item.callId === CALL_ID
      ) ?? -1

    const converted = applyAgentEventToChatMessage(surrounded, lifecycle('running'))
    const replayed = applyAgentEventToChatMessage(converted, lifecycle('running'))
    const timeline = replayed.agentRun?.timeline ?? []

    expect(genericIndex).toBe(1)
    expect(timeline).toEqual([
      {
        id: 'message-stream-before-mcp',
        type: 'message',
        content: 'Before MCP',
        streamId: 'before-mcp'
      },
      {
        id: `mcp-invocation-${INVOCATION_ID}`,
        type: 'mcp_tool_call',
        invocationId: INVOCATION_ID
      },
      {
        id: 'message-stream-after-mcp',
        type: 'message',
        content: 'After MCP',
        streamId: 'after-mcp'
      }
    ])
    expect(timeline.findIndex((item) => item.type === 'mcp_tool_call')).toBe(genericIndex)
    expect(timeline.filter((item) => item.type === 'mcp_tool_call')).toHaveLength(1)
    expect(timeline.some((item) => item.type === 'tool_call' && item.callId === CALL_ID)).toBe(
      false
    )
  })

  it('projects lifecycle fields explicitly and does not retain future wire fields', () => {
    const event = lifecycle('running', { displayReason: 'Read the fixture inventory' })
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
      displayReason: 'Read the fixture inventory',
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

  it('enriches a lifecycle-first invocation with approval metadata without regressing state', () => {
    const running = applyAgentEventToChatMessage(message(), lifecycle('running'))
    const withApproval = applyAgentEventToChatMessage(running, {
      type: 'approval_required',
      runId: RUN_ID,
      action: { type: 'mcp_tool_call', approval: approval() }
    })

    expect(withApproval.agentRun?.mcpInvocations?.[0]).toMatchObject({
      state: 'running',
      scope: { type: 'user' },
      displayReason: 'Read the fixture inventory'
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
      traceSequence: 2,
      identity: { type: 'mcp', provenance: approval().identity.provenance },
      call: {
        id: CALL_ID,
        tool: 'model-visible-name-without-an-mcp-prefix',
        args: { value: SECRET_CANARY },
        approvalStatus: 'approved',
        reason: null
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
                approvalStatus: 'required',
                reason: null
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
      'Use a different directory\u202E'
    )

    expect(rejected.agentRun?.toolCalls).toEqual([])
    expect(rejected.agentRun?.toolResults).toEqual([])
    expect(rejected.agentRun?.timeline.some((item) => item.type === 'tool_call')).toBe(false)
    expect(rejected.agentRun?.mcpInvocations?.[0].rejectionReason).toBe('Use a different directory')
    expect(JSON.stringify(rejected.agentRun)).not.toContain(SECRET_CANARY)

    const settled = applyAgentEventToChatMessage(
      rejected,
      lifecycle('rejected', {
        outcome: 'rejected',
        errorCode: 'mcp.approval_rejected'
      })
    )
    expect(settled.agentRun?.mcpInvocations?.[0]).toMatchObject({
      state: 'rejected',
      rejectionReason: 'Use a different directory'
    })
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

  it('keeps bounded user rejection guidance beside the authoritative MCP lifecycle', () => {
    const action = { type: 'mcp_tool_call' as const, approval: approval() }
    const waiting = applyAgentEventToChatMessage(message(), {
      type: 'approval_required',
      runId: RUN_ID,
      action
    })
    const rejected = applyAgentActionExecutionToChatMessage(
      waiting,
      {
        actionId: ACTION_ID,
        actionType: 'mcp_tool_call',
        toolName: 'provider-safe-name',
        status: 'rejected',
        agentOutput: {
          status: 'running',
          content: '',
          runId: RUN_ID,
          events: [
            lifecycle('rejected', {
              outcome: 'rejected',
              errorCode: 'mcp.approval_rejected'
            })
          ],
          toolDefinitions: [],
          proposedActions: []
        }
      },
      'Use another directory\u202E'
    )

    expect(rejected.agentRun?.mcpInvocations?.[0]).toMatchObject({
      state: 'rejected',
      rejectionReason: 'Use another directory'
    })
    expect(rejected.agentRun?.toolResults).toEqual([])
  })

  it('does not infer MCP identity from an mcp__ model name', () => {
    const generic = applyAgentEventToChatMessage(message(), {
      type: 'tool_call',
      runId: RUN_ID,
      traceSequence: 0,
      identity: { type: 'unregistered', toolName: 'mcp__spoofed__name' },
      call: {
        id: 'ordinary-tool-call',
        tool: 'mcp__spoofed__name',
        args: { visibleGenericArgument: true },
        approvalStatus: 'not_required',
        reason: null
      }
    })

    expect(generic.agentRun?.toolCalls).toHaveLength(1)
    expect(generic.agentRun?.timeline).toContainEqual({
      id: 'tool-call-ordinary-tool-call',
      type: 'tool_call',
      callId: 'ordinary-tool-call',
      identity: { type: 'unregistered', toolName: 'mcp__spoofed__name' },
      traceSequence: 0
    })
    expect(generic.agentRun?.mcpInvocations).toEqual([])
  })

  it('preserves the durable Tool trace sequence through approval projections', () => {
    const announced = applyAgentEventToChatMessage(message(), {
      type: 'tool_call',
      runId: RUN_ID,
      traceSequence: 9,
      identity: { type: 'builtin', toolName: 'apply_patch' },
      call: {
        id: 'file-change-call',
        tool: 'apply_patch',
        args: { action: 'commit', transactionId: 'file-change-1', expectedDraftRevision: 1 },
        approvalStatus: 'required',
        reason: null
      }
    })
    const waiting = applyAgentEventToChatMessage(announced, {
      type: 'approval_required',
      runId: RUN_ID,
      action: {
        type: 'file_change',
        fileChange: {
          schemaVersion: 1,
          id: 'file-change-call',
          transactionId: 'file-change-1',
          operation: 'update',
          updateStrategy: 'rewrite',
          filePath: 'src/main.ts',
          inlineDiff: null,
          baseRevision: 'sha256-base',
          summary: null,
          additions: 1,
          deletions: 0,
          lineCount: 1,
          byteCount: 12,
          approvalStatus: 'required'
        }
      }
    })

    expect(waiting.agentRun?.timeline).toContainEqual({
      id: 'tool-call-file-change-call',
      type: 'tool_call',
      callId: 'file-change-call',
      identity: { type: 'builtin', toolName: 'apply_patch' },
      traceSequence: 9
    })
  })

  it('marks lifecycle events as durable conversation changes', () => {
    expect(shouldTouchConversationForAgentEvent(lifecycle('running'))).toBe(true)
  })

  it('settles a dispatched MCP invocation as outcome unknown when its parent run stops', () => {
    const running = applyAgentEventToChatMessage(message(), lifecycle('running'))
    const stopped = applyAgentEventToChatMessage(running, {
      type: 'done',
      runId: RUN_ID,
      success: false,
      status: 'cancelled',
      proposedActions: []
    })

    expect(stopped.status).toBe('sent')
    expect(stopped.agentRun?.status).toBe('cancelled')
    expect(stopped.agentRun?.mcpInvocations?.[0]).toMatchObject({
      state: 'outcome_unknown',
      outcome: 'outcome_unknown',
      dispatchCertainty: 'possibly_dispatched',
      errorCode: 'mcp.tool_outcome_unknown'
    })
  })

  it('conservatively settles a stale pre-dispatch projection when its parent run stops', () => {
    const waiting = applyAgentEventToChatMessage(message(), {
      type: 'approval_required',
      runId: RUN_ID,
      action: { type: 'mcp_tool_call', approval: approval() }
    })
    const stopped = applyAgentEventToChatMessage(waiting, {
      type: 'done',
      runId: RUN_ID,
      success: false,
      status: 'cancelled',
      proposedActions: []
    })

    expect(stopped.agentRun?.mcpInvocations?.[0]).toMatchObject({
      state: 'outcome_unknown',
      outcome: 'outcome_unknown',
      dispatchCertainty: 'possibly_dispatched',
      errorCode: 'mcp.tool_outcome_unknown'
    })
  })

  it('never lets late buffered activity resurrect a terminal parent run', () => {
    const stopped = applyAgentEventToChatMessage(message(), {
      type: 'done',
      runId: RUN_ID,
      success: false,
      status: 'cancelled',
      proposedActions: []
    })
    const lateDelta = applyAgentEventToChatMessage(stopped, {
      type: 'message_delta',
      runId: RUN_ID,
      streamId: 'late-stream',
      delta: SECRET_CANARY
    })
    const lateRunning = applyAgentEventToChatMessage(lateDelta, {
      type: 'state',
      runId: RUN_ID,
      state: {
        status: 'running',
        activeRunId: RUN_ID,
        lastError: null,
        updatedAt: 20
      }
    })

    expect(lateRunning).toEqual(stopped)
    expect(JSON.stringify(lateRunning)).not.toContain(SECRET_CANARY)
    expect(lateRunning.agentRun?.status).toBe('cancelled')
  })
})
