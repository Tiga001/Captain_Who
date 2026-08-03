import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  parseAgentActionExecutionOutputForHost,
  parseAgentEventForHost,
  parseAgentMcpToolApproval,
  parseAgentMcpToolInvocationEvent,
  parsePendingAgentActionSnapshotsForHost
} from './agentMcpParsers'

const serverId = 'ce18d23c-e74f-4e89-8695-ce1e7c60ec92'
const actionId = '94c2f39c-ddaa-49bb-a3ef-8756053d68c8'
const invocationId = 'a8a6102c-8ad6-45d5-bb0d-3e4f0ad2a30f'
const callId = `tc1_${'a'.repeat(43)}`
const rendererGolden = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/agent-mcp-renderer-contract-v1.json'),
    'utf8'
  )
)
const approval = {
  identity: {
    actionId,
    invocationId,
    runId: 'run-owned',
    callId,
    provenance: {
      serverId,
      scope: { type: 'user' },
      rawToolName: 'echo_text',
      modelToolName: 'mcp__owned_fixture__echo_text',
      configEpoch: '41818332-0842-4d2e-808f-175b70eb4628',
      registryRevision: 7,
      configDigest: 'a'.repeat(64),
      catalogGeneration: 2,
      catalogDigest: 'b'.repeat(64),
      catalogSchemaDigest: 'c'.repeat(64),
      schemaDigest: 'd'.repeat(64),
      schemaNormalizerVersion: 3
    }
  },
  call: {
    id: callId,
    tool: 'mcp__owned_fixture__echo_text',
    args: {},
    approvalStatus: 'required'
  },
  summary: {
    serverId,
    serverDisplayName: 'Owned fixture',
    scope: { type: 'user' },
    rawToolName: 'echo_text',
    modelToolName: 'mcp__owned_fixture__echo_text',
    displayReason: 'Read the requested fixture data',
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
  createdAt: 1_753_843_200_000,
  expiresAt: 1_753_844_100_000
} as const

const invocation = {
  actionId,
  invocationId,
  callId,
  serverId,
  serverDisplayName: 'Owned fixture',
  rawToolName: 'echo_text',
  modelToolName: 'mcp__owned_fixture__echo_text',
  displayReason: 'Read the requested fixture data',
  external: true,
  state: 'running',
  dispatchCertainty: 'possibly_dispatched',
  outputTruncated: false
} as const

const invocationDiagnostics = {
  schemaVersion: 1,
  argumentEncodedBytes: 24,
  argumentValueCount: 2,
  argumentMaxDepth: 2
} as const

const resultSizeSummary = {
  contentBlockCount: 3,
  textBytes: 18,
  structuredBytes: 12,
  omittedBlockCount: 1,
  omittedEncodedBytes: 128
} as const

describe('Round 4 MCP Agent contract', () => {
  it('round-trips the shared Rust/TypeScript Renderer-safe golden events', () => {
    expect(rendererGolden.schemaVersion).toBe(1)
    expect(parseAgentEventForHost(rendererGolden.approvalRequired)).toEqual(
      rendererGolden.approvalRequired
    )
    expect(parseAgentEventForHost(rendererGolden.lifecycle)).toEqual(rendererGolden.lifecycle)
    expect(parseAgentEventForHost(rendererGolden.done)).toEqual(rendererGolden.done)

    const serialized = JSON.stringify(rendererGolden)
    for (const forbidden of [
      'argumentsDigest',
      'rawArguments',
      'rawResult',
      'structuredContent',
      'stderr',
      'payloadRef',
      'ciphertext',
      'toolDefinitions'
    ]) {
      expect(serialized).not.toContain(forbidden)
    }
    expect(serialized).not.toContain('"type":"tool_call"')
    expect(serialized).not.toContain('"type":"tool_result"')
  })

  it('parses a safe typed approval and keeps the Tool args projection empty', () => {
    expect(parseAgentMcpToolApproval(approval)).toEqual(approval)
    expect(() =>
      parseAgentMcpToolApproval({
        ...approval,
        identity: { ...approval.identity, argumentsDigest: 'e'.repeat(64) }
      })
    ).toThrow(/unexpected field argumentsDigest/)
  })

  it.each([
    ['rawArguments', 'fixed-canary-raw-arguments'],
    ['payloadRef', 'fixed-canary-payload-reference'],
    ['ciphertext', 'fixed-canary-ciphertext'],
    ['stderr', 'fixed-canary-stderr'],
    ['environment', { TOKEN: 'fixed-canary-token' }]
  ])('rejects forbidden approval field %s', (field, value) => {
    expect(() => parseAgentMcpToolApproval({ ...approval, [field]: value })).toThrow(
      /unexpected field/
    )
  })

  it('rejects a non-empty safe call projection and mismatched frozen identity', () => {
    expect(() =>
      parseAgentMcpToolApproval({
        ...approval,
        call: { ...approval.call, args: { value: 'fixed-canary-must-not-cross' } }
      })
    ).toThrow(/unexpected field/)
    expect(() =>
      parseAgentMcpToolApproval({
        ...approval,
        summary: { ...approval.summary, rawToolName: 'different_tool' }
      })
    ).toThrow(/summary must match/)
  })

  it('requires high-entropy invocation ids and mode-consistent call status', () => {
    expect(() =>
      parseAgentMcpToolApproval({
        ...approval,
        identity: { ...approval.identity, actionId: 'action-owned' }
      })
    ).toThrow(/UUIDv4/)
    expect(() =>
      parseAgentMcpToolApproval({
        ...approval,
        identity: { ...approval.identity, invocationId: actionId }
      })
    ).toThrow(/must be distinct/)
    expect(() => parseAgentMcpToolApproval({ ...approval, approvalMode: 'deny' })).toThrow(
      /mode and call status/
    )
    expect(() =>
      parseAgentMcpToolApproval({
        ...approval,
        call: { ...approval.call, approvalStatus: 'approved' }
      })
    ).toThrow(/mode and call status/)
    expect(
      parseAgentMcpToolApproval({
        ...approval,
        approvalMode: 'auto',
        call: { ...approval.call, approvalStatus: 'approved' }
      })
    ).toMatchObject({ approvalMode: 'auto', call: { approvalStatus: 'approved' } })
  })

  it('parses only the safe lifecycle event fields', () => {
    expect(parseAgentMcpToolInvocationEvent(invocation)).toEqual(invocation)
    expect(
      parseAgentEventForHost({
        type: 'mcp_tool_invocation_state_changed',
        runId: 'run-owned',
        invocation
      })
    ).toEqual({
      type: 'mcp_tool_invocation_state_changed',
      runId: 'run-owned',
      invocation
    })
  })

  it('parses value-free invocation diagnostics for active and completed calls', () => {
    expect(
      parseAgentMcpToolInvocationEvent({
        ...invocation,
        diagnostics: invocationDiagnostics
      })
    ).toEqual({
      ...invocation,
      diagnostics: invocationDiagnostics
    })

    const completed = {
      ...invocation,
      state: 'completed',
      dispatchCertainty: 'response_received',
      outcome: 'succeeded',
      isError: false,
      durationMs: 25,
      diagnostics: {
        ...invocationDiagnostics,
        result: resultSizeSummary
      }
    } as const
    expect(parseAgentMcpToolInvocationEvent(completed)).toEqual(completed)

    const toolError = {
      ...completed,
      outcome: 'tool_error',
      isError: true,
      errorCode: 'mcp.tool_error',
      diagnostics: {
        ...invocationDiagnostics,
        result: resultSizeSummary,
        failureStage: 'server_response'
      }
    } as const
    expect(parseAgentMcpToolInvocationEvent(toolError)).toEqual(toolError)
  })

  it('accepts legacy invocation events without diagnostics', () => {
    expect(parseAgentMcpToolInvocationEvent(invocation)).toEqual(invocation)
    expect(
      parseAgentMcpToolInvocationEvent({
        ...invocation,
        state: 'completed',
        dispatchCertainty: 'response_received',
        outcome: 'succeeded',
        isError: false,
        durationMs: 25
      })
    ).not.toHaveProperty('diagnostics')
  })

  it.each([
    ['rawArguments', { secret: 'fixed-canary-raw-arguments' }],
    ['rawResult', 'fixed-canary-raw-result'],
    ['structuredContent', { secret: 'fixed-canary-structured-content' }],
    ['stderr', 'fixed-canary-stderr'],
    ['payloadRef', 'fixed-canary-payload-reference'],
    ['unknownField', 'fixed-canary-unknown-field']
  ])('rejects forbidden diagnostics field %s', (field, value) => {
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...invocation,
        diagnostics: {
          ...invocationDiagnostics,
          [field]: value
        }
      })
    ).toThrow(/unexpected field/)
  })

  it.each([
    ['rawText', 'fixed-canary-raw-text'],
    ['preview', 'fixed-canary-preview'],
    ['mimeType', 'application/fixed-canary'],
    ['unknownField', 'fixed-canary-unknown-field']
  ])('rejects forbidden result diagnostics field %s', (field, value) => {
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...invocation,
        state: 'completed',
        dispatchCertainty: 'response_received',
        outcome: 'succeeded',
        isError: false,
        durationMs: 25,
        diagnostics: {
          ...invocationDiagnostics,
          result: {
            ...resultSizeSummary,
            [field]: value
          }
        }
      })
    ).toThrow(/unexpected field/)
  })

  it('accepts diagnostics exactly at every numeric safety limit', () => {
    const event = {
      ...invocation,
      state: 'completed',
      dispatchCertainty: 'response_received',
      outcome: 'succeeded',
      isError: false,
      durationMs: 25,
      diagnostics: {
        schemaVersion: 1,
        argumentEncodedBytes: 64 * 1024,
        argumentValueCount: 4096,
        argumentMaxDepth: 32,
        result: {
          contentBlockCount: 128,
          textBytes: 4 * 1024 * 1024,
          structuredBytes: 4 * 1024 * 1024,
          omittedBlockCount: 128,
          omittedEncodedBytes: 4 * 1024 * 1024
        }
      }
    } as const
    expect(parseAgentMcpToolInvocationEvent(event)).toEqual(event)
  })

  it('rejects diagnostics outside numeric and structural safety limits', () => {
    const invalidDiagnostics = [
      { ...invocationDiagnostics, schemaVersion: 2 },
      { ...invocationDiagnostics, argumentEncodedBytes: 64 * 1024 + 1 },
      { ...invocationDiagnostics, argumentValueCount: 4097 },
      { ...invocationDiagnostics, argumentMaxDepth: 33 },
      { ...invocationDiagnostics, argumentEncodedBytes: -1 },
      { ...invocationDiagnostics, argumentValueCount: 1.5 },
      {
        ...invocationDiagnostics,
        result: { ...resultSizeSummary, contentBlockCount: 129 }
      },
      {
        ...invocationDiagnostics,
        result: { ...resultSizeSummary, omittedBlockCount: 129 }
      },
      {
        ...invocationDiagnostics,
        result: { ...resultSizeSummary, textBytes: 4 * 1024 * 1024 + 1 }
      },
      {
        ...invocationDiagnostics,
        result: { ...resultSizeSummary, structuredBytes: 4 * 1024 * 1024 + 1 }
      },
      {
        ...invocationDiagnostics,
        result: { ...resultSizeSummary, omittedEncodedBytes: 4 * 1024 * 1024 + 1 }
      },
      {
        ...invocationDiagnostics,
        result: { ...resultSizeSummary, contentBlockCount: 0, omittedBlockCount: 1 }
      },
      {
        ...invocationDiagnostics,
        result: { ...resultSizeSummary, textBytes: Number.MAX_SAFE_INTEGER + 1 }
      }
    ]

    for (const diagnostics of invalidDiagnostics) {
      expect(() =>
        parseAgentMcpToolInvocationEvent({
          ...invocation,
          state: 'completed',
          dispatchCertainty: 'response_received',
          outcome: 'succeeded',
          isError: false,
          durationMs: 25,
          diagnostics
        })
      ).toThrow()
    }
  })

  it('rejects unknown failure-stage values before lifecycle projection', () => {
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...invocation,
        state: 'failed',
        dispatchCertainty: 'definitely_not_dispatched',
        outcome: 'transport_error',
        isError: true,
        errorCode: 'mcp.tool_unavailable',
        durationMs: 25,
        diagnostics: {
          ...invocationDiagnostics,
          failureStage: 'server_supplied_stage'
        }
      })
    ).toThrow(/failureStage/)
  })

  it('enforces result and failure-stage consistency for every lifecycle class', () => {
    const completedSuccess = {
      ...invocation,
      state: 'completed',
      dispatchCertainty: 'response_received',
      outcome: 'succeeded',
      isError: false,
      durationMs: 25
    } as const
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...invocation,
        diagnostics: { ...invocationDiagnostics, result: resultSizeSummary }
      })
    ).toThrow(/diagnostics contradict/)
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...invocation,
        diagnostics: { ...invocationDiagnostics, failureStage: 'dispatch' }
      })
    ).toThrow(/diagnostics contradict/)
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...completedSuccess,
        diagnostics: invocationDiagnostics
      })
    ).toThrow(/diagnostics contradict/)
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...completedSuccess,
        diagnostics: {
          ...invocationDiagnostics,
          result: resultSizeSummary,
          failureStage: 'transport'
        }
      })
    ).toThrow(/diagnostics contradict/)

    const completedToolError = {
      ...completedSuccess,
      outcome: 'tool_error',
      isError: true,
      errorCode: 'mcp.tool_error'
    } as const
    expect(
      parseAgentMcpToolInvocationEvent({
        ...completedToolError,
        diagnostics: {
          ...invocationDiagnostics,
          result: resultSizeSummary,
          failureStage: 'server_response'
        }
      })
    ).toMatchObject({ state: 'completed', outcome: 'tool_error' })
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...completedToolError,
        diagnostics: {
          ...invocationDiagnostics,
          result: resultSizeSummary,
          failureStage: 'transport'
        }
      })
    ).toThrow(/diagnostics contradict/)

    const expired = {
      ...invocation,
      state: 'expired',
      dispatchCertainty: 'definitely_not_dispatched',
      outcome: 'expired',
      errorCode: 'mcp.approval_payload_expired'
    } as const
    expect(
      parseAgentMcpToolInvocationEvent({
        ...expired,
        diagnostics: { ...invocationDiagnostics, failureStage: 'approval_payload' }
      })
    ).toMatchObject({ state: 'expired' })
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...expired,
        diagnostics: { ...invocationDiagnostics, failureStage: 'policy' }
      })
    ).toThrow(/diagnostics contradict/)

    const policyDenied = {
      ...invocation,
      state: 'policy_denied',
      dispatchCertainty: 'definitely_not_dispatched',
      outcome: 'policy_denied',
      errorCode: 'mcp.approval_policy_denied'
    } as const
    expect(
      parseAgentMcpToolInvocationEvent({
        ...policyDenied,
        diagnostics: { ...invocationDiagnostics, failureStage: 'policy' }
      })
    ).toMatchObject({ state: 'policy_denied' })
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...policyDenied,
        diagnostics: { ...invocationDiagnostics, failureStage: 'approval_payload' }
      })
    ).toThrow(/diagnostics contradict/)

    const cancelled = {
      ...invocation,
      state: 'cancelled',
      dispatchCertainty: 'definitely_not_dispatched',
      outcome: 'cancelled',
      errorCode: 'mcp.tool_cancelled'
    } as const
    expect(
      parseAgentMcpToolInvocationEvent({
        ...cancelled,
        diagnostics: { ...invocationDiagnostics, failureStage: 'preflight' }
      })
    ).toMatchObject({ state: 'cancelled' })
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...cancelled,
        diagnostics: { ...invocationDiagnostics, failureStage: 'transport' }
      })
    ).toThrow(/diagnostics contradict/)

    const rejected = {
      ...invocation,
      state: 'rejected',
      dispatchCertainty: 'definitely_not_dispatched',
      outcome: 'rejected',
      errorCode: 'mcp.approval_rejected'
    } as const
    expect(
      parseAgentMcpToolInvocationEvent({
        ...rejected,
        diagnostics: invocationDiagnostics
      })
    ).toMatchObject({ state: 'rejected' })
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...rejected,
        diagnostics: { ...invocationDiagnostics, failureStage: 'policy' }
      })
    ).toThrow(/diagnostics contradict/)

    const failed = {
      ...invocation,
      state: 'failed',
      dispatchCertainty: 'definitely_not_dispatched',
      outcome: 'transport_error',
      isError: true,
      errorCode: 'mcp.tool_unavailable',
      durationMs: 25
    } as const
    expect(
      parseAgentMcpToolInvocationEvent({
        ...failed,
        diagnostics: { ...invocationDiagnostics, failureStage: 'transport' }
      })
    ).toMatchObject({ state: 'failed' })
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...failed,
        diagnostics: invocationDiagnostics
      })
    ).toThrow(/diagnostics contradict/)
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...failed,
        diagnostics: {
          ...invocationDiagnostics,
          result: resultSizeSummary,
          failureStage: 'transport'
        }
      })
    ).toThrow(/diagnostics contradict/)

    const outcomeUnknown = {
      ...invocation,
      state: 'outcome_unknown',
      dispatchCertainty: 'possibly_dispatched',
      outcome: 'outcome_unknown',
      errorCode: 'mcp.tool_outcome_unknown'
    } as const
    expect(
      parseAgentMcpToolInvocationEvent({
        ...outcomeUnknown,
        diagnostics: { ...invocationDiagnostics, failureStage: 'transport' }
      })
    ).toMatchObject({ state: 'outcome_unknown' })
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...outcomeUnknown,
        diagnostics: invocationDiagnostics
      })
    ).toThrow(/diagnostics contradict/)
  })

  it.each(['rawArguments', 'rawResult', 'structuredContent', 'stderr', 'payloadRef'])(
    'rejects forbidden lifecycle field %s',
    (field) => {
      expect(() =>
        parseAgentMcpToolInvocationEvent({
          ...invocation,
          [field]: 'fixed-canary-must-not-cross'
        })
      ).toThrow(/unexpected field/)
    }
  )

  it.each(['\n', '\u202e', '\u2066', '\u2028', '\u2029'])(
    'rejects disallowed display character %j in MCP lifecycle text',
    (disallowed) => {
      expect(() =>
        parseAgentMcpToolInvocationEvent({
          ...invocation,
          serverDisplayName: `Owned${disallowed}fixture`
        })
      ).toThrow(/without controls/)
      expect(() =>
        parseAgentMcpToolInvocationEvent({
          ...invocation,
          rawToolName: `echo${disallowed}text`
        })
      ).toThrow(/disallowed control/)
      expect(() =>
        parseAgentMcpToolInvocationEvent({
          ...invocation,
          displayReason: `Read${disallowed}fixture`
        })
      ).toThrow(/disallowed control/)
    }
  )

  it('accepts a missing legacy lifecycle reason and rejects an oversized one', () => {
    const legacyInvocation = { ...invocation }
    delete legacyInvocation.displayReason
    expect(parseAgentMcpToolInvocationEvent(legacyInvocation)).toEqual(legacyInvocation)
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...invocation,
        displayReason: '界'.repeat(171)
      })
    ).toThrow(/exceeded 512 UTF-8 bytes/)
  })

  it('rejects contradictory invocation lifecycle combinations', () => {
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...invocation,
        state: 'pending_approval',
        dispatchCertainty: 'response_received'
      })
    ).toThrow(/inconsistent/)
    expect(() =>
      parseAgentMcpToolInvocationEvent({
        ...invocation,
        state: 'outcome_unknown',
        dispatchCertainty: 'definitely_not_dispatched',
        outcome: 'outcome_unknown',
        errorCode: 'mcp.outcome_unknown'
      })
    ).toThrow(/inconsistent/)
    expect(
      parseAgentMcpToolInvocationEvent({
        ...invocation,
        state: 'completed',
        dispatchCertainty: 'response_received',
        outcome: 'succeeded',
        isError: false,
        durationMs: 25
      })
    ).toMatchObject({ state: 'completed', outcome: 'succeeded' })
  })

  it('strictly parses MCP approval_required and done actions', () => {
    expect(
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-owned',
        action: { type: 'mcp_tool_call', approval }
      })
    ).toMatchObject({ type: 'approval_required' })
    expect(
      parseAgentEventForHost({
        type: 'done',
        runId: 'run-owned',
        success: true,
        status: 'waiting_for_approval',
        proposedActions: [{ type: 'mcp_tool_call', approval }]
      })
    ).toMatchObject({
      type: 'done',
      proposedActions: [{ type: 'mcp_tool_call' }]
    })
  })

  it('bounds MCP done content/actions and rejects mixed or cross-run approvals', () => {
    const done = {
      type: 'done',
      runId: 'run-owned',
      success: true,
      status: 'waiting_for_approval',
      proposedActions: [{ type: 'mcp_tool_call', approval }]
    }
    expect(() =>
      parseAgentEventForHost({
        ...done,
        content: 'x'.repeat(1024 * 1024 + 1)
      })
    ).toThrow(/exceeded 1048576 UTF-8 bytes/)
    expect(() =>
      parseAgentEventForHost({
        ...done,
        proposedActions: Array.from({ length: 1025 }, () => ({
          type: 'mcp_tool_call',
          approval
        }))
      })
    ).toThrow(/at most 1024 items/)
    expect(() =>
      parseAgentEventForHost({
        ...done,
        proposedActions: [
          { type: 'mcp_tool_call', approval },
          { type: 'run_command', command: 'must-not-be-filtered' }
        ]
      })
    ).toThrow(/mixed MCP and non-MCP/)
    expect(() =>
      parseAgentEventForHost({
        ...done,
        proposedActions: [
          {
            type: 'mcp_tool_call',
            approval: {
              ...approval,
              identity: { ...approval.identity, runId: 'run-other' }
            }
          }
        ]
      })
    ).toThrow(/runId must match enclosing runId/)
  })

  it('strictly parses pending MCP approvals without exposing sealed payload data', () => {
    const pending = {
      actionId,
      actionType: 'mcp_tool_call',
      toolName: 'mcp__owned_fixture__echo_text',
      toolCallId: callId,
      runId: 'run-owned',
      conversationId: 'conversation-owned',
      assistantMessageId: 'assistant-owned',
      action: { type: 'mcp_tool_call', approval },
      createdAt: 1_753_843_200_000,
      status: 'pending'
    }

    expect(parsePendingAgentActionSnapshotsForHost([pending])).toEqual([pending])
    expect(() =>
      parsePendingAgentActionSnapshotsForHost([
        {
          ...pending,
          payloadRef: 'fixed-canary-payload-reference'
        }
      ])
    ).toThrow(/unexpected field/)
  })

  it('projects MCP execution output onto lifecycle-only Renderer-safe data', () => {
    const output = parseAgentActionExecutionOutputForHost({
      actionId,
      actionType: 'mcp_tool_call',
      toolName: 'mcp__owned_fixture__echo_text',
      status: 'approved',
      toolResult: {
        callId,
        tool: 'mcp__owned_fixture__echo_text',
        ok: true,
        result: 'fixed-canary-raw-result'
      },
      agentOutput: {
        content: 'Execution completed.',
        status: 'completed',
        runId: 'run-owned',
        events: [
          {
            type: 'tool_result',
            runId: 'run-owned',
            result: {
              callId,
              tool: 'mcp__owned_fixture__echo_text',
              ok: true,
              result: 'fixed-canary-event-result'
            }
          },
          {
            type: 'mcp_tool_invocation_state_changed',
            runId: 'run-owned',
            invocation: {
              ...invocation,
              state: 'completed',
              dispatchCertainty: 'response_received',
              outcome: 'succeeded',
              isError: false,
              durationMs: 25
            }
          }
        ],
        toolDefinitions: [
          {
            name: 'mcp__owned_fixture__echo_text',
            description: 'fixed-canary-server-description',
            inputSchema: { secret: 'fixed-canary-schema' },
            safety: 'external',
            requiresWorkspace: false,
            requiresApproval: true,
            approvalMode: 'always'
          }
        ],
        proposedActions: [{ type: 'mcp_tool_call', approval }]
      }
    })

    expect(output.toolResult).toBeUndefined()
    expect(output.agentOutput.toolDefinitions).toEqual([])
    expect(output.agentOutput.events).toHaveLength(1)
    expect(output.agentOutput.proposedActions).toEqual([{ type: 'mcp_tool_call', approval }])
    expect(JSON.stringify(output)).not.toContain('fixed-canary')
  })

  it('bounds MCP execution output and fails closed on mixed or cross-run approvals', () => {
    const executionOutput = {
      actionId,
      actionType: 'mcp_tool_call',
      toolName: 'mcp__owned_fixture__echo_text',
      status: 'approved',
      agentOutput: {
        content: 'Execution completed.',
        status: 'completed',
        runId: 'run-owned',
        events: [],
        toolDefinitions: [],
        proposedActions: [{ type: 'mcp_tool_call', approval }]
      }
    }
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        ...executionOutput,
        agentOutput: {
          ...executionOutput.agentOutput,
          content: 'x'.repeat(1024 * 1024 + 1)
        }
      })
    ).toThrow(/exceeded 1048576 UTF-8 bytes/)
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        ...executionOutput,
        agentOutput: {
          ...executionOutput.agentOutput,
          proposedActions: Array.from({ length: 1025 }, () => ({
            type: 'mcp_tool_call',
            approval
          }))
        }
      })
    ).toThrow(/at most 1024 items/)
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        ...executionOutput,
        agentOutput: {
          ...executionOutput.agentOutput,
          proposedActions: [
            { type: 'mcp_tool_call', approval },
            { type: 'run_command', command: 'must-not-be-filtered' }
          ]
        }
      })
    ).toThrow(/mixed MCP and non-MCP/)
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        ...executionOutput,
        agentOutput: {
          ...executionOutput.agentOutput,
          proposedActions: [
            {
              type: 'mcp_tool_call',
              approval: {
                ...approval,
                identity: { ...approval.identity, runId: 'run-other' }
              }
            }
          ]
        }
      })
    ).toThrow(/runId must match enclosing runId/)
    const crossRunApproval = {
      ...approval,
      identity: { ...approval.identity, runId: 'run-other' }
    }
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        ...executionOutput,
        agentOutput: {
          ...executionOutput.agentOutput,
          events: [
            {
              type: 'approval_required',
              runId: 'run-other',
              action: { type: 'mcp_tool_call', approval: crossRunApproval }
            }
          ]
        }
      })
    ).toThrow(/nested MCP event runId must match output runId/)
  })
})
