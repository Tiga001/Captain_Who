import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import type { AgentEvent } from './agent'
import {
  parseAgentObserverEventEnvelope,
  type AgentObserverEventEnvelope
} from './agentCollaboration'
import { parseAgentEventForHost } from './agentParsers/events'

const runId = 'run-owned'

const rendererGolden = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/agent-mcp-renderer-contract-v1.json'),
    'utf8'
  )
) as { lifecycle: AgentEvent }

const fileChange = {
  schemaVersion: 1,
  id: 'file-change-observer-contract',
  transactionId: 'file-change-transaction-observer-contract',
  operation: 'update',
  updateStrategy: null,
  filePath: 'README.md',
  inlineDiff: { patch: '@@ -1 +1 @@\n-old\n+new', truncated: false },
  baseRevision: 'content-sha256-v1:base',
  summary: 'Update the heading',
  additions: 1,
  deletions: 1,
  lineCount: 1,
  byteCount: 3,
  approvalStatus: 'required'
} as const

/**
 * These are the event families that the former observer-only parser omitted even though Core
 * legitimately forwards them. Keeping the fixtures typed makes additions or field changes fail at
 * compile time; passing every value through both entry points prevents the two parsers drifting.
 */
const formerlyDroppedEvents = [
  { type: 'started', runId, toolDefinitions: [] },
  {
    type: 'tool_set_changed',
    runId,
    stableRevision: 'stable-v1',
    dynamicRevision: 'dynamic-v1',
    effectiveRevision: 'effective-v1',
    toolDefinitions: []
  },
  {
    type: 'tool_input_progress',
    runId,
    streamId: 'stream-1',
    attempt: 1,
    toolCallIndex: 0,
    toolCallId: 'call-1',
    tool: 'read_file',
    receivedBytes: 42
  },
  {
    type: 'file_change_preview_updated',
    runId,
    preview: {
      schemaVersion: 1,
      previewId: 'preview-1',
      streamId: 'stream-1',
      attempt: 1,
      toolCallIndex: 0,
      toolCallId: 'call-1',
      transactionId: 'draft-1',
      filePath: 'README.md',
      additions: 1,
      deletions: 1,
      lineCount: 1,
      byteCount: 3,
      generatedBytes: 3,
      contentOffsetBytes: 0,
      contentDelta: 'new',
      updatedAt: 10
    }
  },
  {
    type: 'file_change_preview_cleared',
    runId,
    streamId: 'stream-1',
    attempt: 1
  },
  {
    type: 'guidance_queued',
    runId,
    guidanceId: 'guidance-1',
    clientMessageId: 'client-message-1',
    content: 'Continue with the review.',
    attachments: [],
    createdAt: 10
  },
  {
    type: 'guidance_applied',
    runId,
    guidanceId: 'guidance-1',
    clientMessageId: 'client-message-1',
    content: 'Continue with the review.',
    attachments: [],
    createdAt: 10,
    sequence: 1
  },
  {
    type: 'guidance_rejected',
    runId,
    guidanceId: 'guidance-1',
    clientMessageId: 'client-message-1',
    content: 'Continue with the review.',
    rejectionCode: 'run_not_steerable',
    message: 'The Run already settled.',
    createdAt: 10
  },
  {
    type: 'todo_updated',
    runId,
    todo: {
      revision: 1,
      items: [
        {
          id: 'todo-1',
          title: 'Review the observer contract',
          status: 'in_progress',
          createdAt: 10,
          updatedAt: 11
        }
      ],
      updatedAt: 11
    }
  },
  {
    type: 'context_window_updated',
    runId,
    conversationId: 'conversation-child',
    modelConfigId: 'model-config-1',
    snapshot: {
      model: 'model-1',
      status: 'within_budget',
      contextWindowTokens: 128_000,
      reservedOutputTokens: 8_000,
      safetyMarginTokens: 1_000,
      inputCapacityTokens: 119_000,
      inputTokens: 1_000,
      costBreakdown: {
        systemTokens: 100,
        toolSchemaTokens: 100,
        summaryTokens: 100,
        worldStateTokens: 100,
        todoTokens: 100,
        providerContinuationTokens: 100,
        recentHistoryTokens: 400,
        totalInputTokens: 1_000
      },
      remainingInputTokens: 118_000
    }
  },
  { type: 'approval_required', runId, action: { type: 'file_change', fileChange } },
  { type: 'file_change_proposed', runId, fileChange }
] satisfies AgentEvent[]

const formerlySupportedEvents = [
  {
    type: 'state',
    runId,
    state: { status: 'running', activeRunId: runId, lastError: null, updatedAt: 10 }
  },
  { type: 'message_delta', runId, streamId: 'stream-1', delta: 'hello' },
  { type: 'message_stream_started', runId, streamId: 'stream-1', attempt: 1 },
  { type: 'message_stream_reset', runId, streamId: 'stream-1', reason: 'retrying_model_request' },
  { type: 'message_stream_committed', runId, streamId: 'stream-1', traceSequence: 1 },
  {
    type: 'llm_retry',
    runId,
    streamId: 'stream-1',
    category: 'network',
    providerCode: 'connection_reset',
    delayMs: 250,
    retryAt: 1_000,
    attempt: 1,
    maxAttempts: 3
  },
  { type: 'message', runId, content: 'Complete response.' },
  {
    type: 'tool_call',
    runId,
    traceSequence: 2,
    identity: { type: 'builtin', toolName: 'read_file' },
    call: {
      id: `tc1_${'a'.repeat(43)}`,
      tool: 'read_file',
      args: { path: 'README.md' },
      approvalStatus: 'not_required',
      reason: 'Inspect the project documentation.'
    }
  },
  {
    type: 'tool_result',
    runId,
    result: {
      callId: `tc1_${'a'.repeat(43)}`,
      tool: 'read_file',
      ok: true,
      result: { content: 'Project' }
    }
  },
  rendererGolden.lifecycle,
  {
    type: 'skill_activated',
    runId,
    activationRevision: `skill-activation-sha256-v1:${'a'.repeat(64)}`,
    activatedBy: 'model',
    skill: {
      id: 'workspace:skill-observer-contract',
      name: 'observer-contract',
      revision: `skill-package-sha256-v3:${'b'.repeat(64)}`,
      source: { kind: 'workspace', id: 'workspace:skill-observer-contract' }
    }
  },
  {
    type: 'file_change_updated',
    runId,
    fileChange: {
      schemaVersion: 1,
      transactionId: 'draft-1',
      conversationId: 'conversation-child',
      projectId: 'project-1',
      filePath: 'README.md',
      operation: 'update',
      updateStrategy: 'modify',
      status: 'drafting',
      baseRevision: 'content-sha256-v1:base',
      additions: 1,
      deletions: 1,
      lineCount: 1,
      byteCount: 3,
      mutationCount: 1,
      nextMutationIndex: 1,
      statsFinal: false,
      summary: null,
      createdAt: 10,
      updatedAt: 11
    }
  },
  { type: 'context_compaction_started', runId, operationId: 'operation-1', traceSequence: 3 },
  {
    type: 'context_compaction_finished',
    runId,
    operationId: 'operation-1',
    outcome: 'applied',
    traceSequence: 4
  },
  {
    type: 'command_started',
    runId,
    conversationId: 'conversation-child',
    assistantMessageId: 'assistant-child',
    projectId: 'project-1',
    callId: 'call-command-1',
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    startedAt: 10
  },
  {
    type: 'command_output',
    runId,
    conversationId: 'conversation-child',
    assistantMessageId: 'assistant-child',
    projectId: 'project-1',
    callId: 'call-command-1',
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    sequence: 1,
    stream: 'stdout',
    output: 'ready\n'
  },
  {
    type: 'command_exited',
    runId,
    conversationId: 'conversation-child',
    assistantMessageId: 'assistant-child',
    projectId: 'project-1',
    callId: 'call-command-1',
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    status: 'exited',
    exitCode: 0,
    endedAt: 11,
    latestSequence: 1,
    outputTruncated: false
  },
  {
    type: 'command_interrupted',
    runId,
    conversationId: 'conversation-child',
    assistantMessageId: 'assistant-child',
    projectId: 'project-1',
    callId: 'call-command-1',
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    endedAt: 11,
    latestSequence: 1,
    outputTruncated: false
  },
  {
    type: 'error',
    runId,
    traceSequence: null,
    message: 'Provider temporarily unavailable.',
    recoverable: true,
    code: 'provider_unavailable'
  },
  { type: 'done', runId, success: true, status: 'completed', content: 'Done.' }
] satisfies AgentEvent[]

const allEventFixtures = [...formerlyDroppedEvents, ...formerlySupportedEvents]
const allEventTypes = [
  'started',
  'tool_set_changed',
  'state',
  'message_delta',
  'message_stream_started',
  'message_stream_reset',
  'message_stream_committed',
  'llm_retry',
  'tool_input_progress',
  'file_change_preview_updated',
  'file_change_preview_cleared',
  'message',
  'guidance_queued',
  'guidance_applied',
  'guidance_rejected',
  'tool_call',
  'tool_result',
  'mcp_tool_invocation_state_changed',
  'todo_updated',
  'skill_activated',
  'file_change_updated',
  'context_window_updated',
  'context_compaction_started',
  'context_compaction_finished',
  'approval_required',
  'file_change_proposed',
  'command_started',
  'command_output',
  'command_exited',
  'command_interrupted',
  'error',
  'done'
] as const satisfies readonly AgentEvent['type'][]

type MissingEventType = Exclude<AgentEvent['type'], (typeof allEventTypes)[number]>
type UnknownEventType = Exclude<(typeof allEventTypes)[number], AgentEvent['type']>
const completeEventTypeCoverage: MissingEventType extends never
  ? UnknownEventType extends never
    ? true
    : never
  : never = true

function envelope(event: AgentEvent): AgentObserverEventEnvelope {
  return {
    schemaVersion: 1,
    rootAgentId: 'agent-root',
    rootConversationId: 'conversation-root',
    agentId: 'agent-child',
    conversationId: 'conversation-child',
    runId,
    assistantMessageId: 'assistant-child',
    event
  }
}

describe('Agent observer event contract', () => {
  it('locks the complete 32-variant AgentEvent set across compile-time and runtime fixtures', () => {
    expect(completeEventTypeCoverage).toBe(true)
    expect(allEventTypes).toHaveLength(32)
    expect(new Set(allEventTypes).size).toBe(32)
    expect(new Set(allEventFixtures.map((event) => event.type))).toEqual(new Set(allEventTypes))
  })

  it.each(allEventFixtures.map((event) => [event.type, event] as const))(
    'uses the canonical Host parser for %s',
    (_type, event) => {
      const canonical = parseAgentEventForHost(event)
      expect(parseAgentObserverEventEnvelope(envelope(event)).event).toEqual(canonical)
    }
  )

  it('requires complete owner-safe FileChange preview fields', () => {
    const current = formerlyDroppedEvents.find(
      (event) => event.type === 'file_change_preview_updated'
    )
    expect(current?.type).toBe('file_change_preview_updated')
    if (current?.type !== 'file_change_preview_updated') throw new Error('preview fixture missing')

    const missingToolCallId: Partial<typeof current.preview> = { ...current.preview }
    delete missingToolCallId.toolCallId
    expect(() => parseAgentEventForHost({ ...current, preview: missingToolCallId })).toThrow(
      /toolCallId/
    )
    expect(() =>
      parseAgentEventForHost({ ...current, preview: { ...current.preview, transactionId: '' } })
    ).toThrow(/transactionId/)
    expect(() =>
      parseAgentEventForHost({ ...current, preview: { ...current.preview, attempt: 0 } })
    ).toThrow(/attempt/)
    expect(() =>
      parseAgentEventForHost({
        ...current,
        preview: { ...current.preview, contentOffsetBytes: 3, generatedBytes: 3 }
      })
    ).toThrow(/content byte cursor/)
  })

  it('binds an Error without a Run identity to the trusted observer envelope', () => {
    const parsed = parseAgentObserverEventEnvelope(
      envelope({
        type: 'error',
        traceSequence: null,
        message: 'Provider temporarily unavailable.',
        recoverable: true
      })
    )

    expect(parsed.event).toEqual({
      type: 'error',
      runId,
      traceSequence: null,
      message: 'Provider temporarily unavailable.',
      recoverable: true
    })
  })

  it('rejects an Error whose explicit Run identity disagrees with the envelope', () => {
    expect(() =>
      parseAgentObserverEventEnvelope(
        envelope({
          type: 'error',
          runId: 'run-forged',
          traceSequence: null,
          message: 'Provider temporarily unavailable.',
          recoverable: true
        })
      )
    ).toThrow(/run identity/i)
  })

  it('rejects nested Conversation and active Run identities that disagree with the envelope', () => {
    expect(() =>
      parseAgentObserverEventEnvelope(
        envelope({
          type: 'context_window_updated',
          runId,
          conversationId: 'conversation-forged',
          modelConfigId: 'model-config-1',
          snapshot: {
            model: 'model-1',
            status: 'within_budget',
            contextWindowTokens: 128_000,
            reservedOutputTokens: 8_000,
            safetyMarginTokens: 1_000,
            inputCapacityTokens: 119_000,
            inputTokens: 1_000,
            costBreakdown: {
              systemTokens: 100,
              toolSchemaTokens: 100,
              summaryTokens: 100,
              worldStateTokens: 100,
              todoTokens: 100,
              providerContinuationTokens: 100,
              recentHistoryTokens: 400,
              totalInputTokens: 1_000
            },
            remainingInputTokens: 118_000
          }
        })
      )
    ).toThrow(/context window identity/i)

    expect(() =>
      parseAgentObserverEventEnvelope(
        envelope({
          type: 'state',
          runId,
          state: {
            status: 'running',
            activeRunId: 'run-forged',
            lastError: null,
            updatedAt: 10
          }
        })
      )
    ).toThrow(/state identity/i)
  })

  it('preserves a non-empty, strictly parsed proposed-action list on Done', () => {
    const event = {
      type: 'done',
      runId,
      success: false,
      status: 'waiting_for_approval',
      content: 'Approval is required.',
      proposedActions: [{ type: 'file_change', fileChange }]
    } satisfies AgentEvent

    expect(parseAgentObserverEventEnvelope(envelope(event)).event).toEqual(event)
  })

  it.each([
    {
      type: 'command',
      command: {
        id: 'command-1',
        command: 'pwd',
        cwd: null,
        timeoutMs: null,
        approvalStatus: 'required',
        riskLevel: null,
        reason: null,
        observe: {}
      }
    },
    {
      type: 'skill_script',
      script: {
        id: 'script-1',
        scriptUri: 'skill://script.py',
        skillId: 'skill-1',
        skillRevision: 'revision-1',
        resourcePath: 'scripts/script.py',
        resourceDigest: 'digest-1',
        source: {
          sourceId: 'installed:user',
          sourceKind: 'installed',
          trust: 'untrusted'
        },
        interpreter: 'python3',
        args: [],
        requirements: {},
        preflight: {},
        timeoutMs: null,
        approvalStatus: 'required',
        reason: null
      }
    },
    {
      type: 'office_operation',
      officeOperation: {
        schemaVersion: 6,
        id: 'office-1',
        semanticArgs: {},
        prepared: {},
        approvalStatus: 'required',
        reason: 'Edit the document.'
      }
    },
    {
      type: 'skill_installation',
      installation: {
        schemaVersion: 1,
        id: 'installation-1',
        installRef: 'install-ref-1',
        preview: {},
        approvalStatus: 'required',
        expiresAt: 10
      }
    }
  ])('rejects a malformed nested projection for $type', (action) => {
    expect(() =>
      parseAgentObserverEventEnvelope(
        envelope({
          type: 'done',
          runId,
          success: false,
          status: 'waiting_for_approval',
          proposedActions: [action]
        } as never)
      )
    ).toThrow()
  })

  it('rejects Renderer-only queued status values that the Rust AgentEvent contract cannot emit', () => {
    expect(() =>
      parseAgentEventForHost({
        type: 'state',
        runId,
        state: { status: 'queued', activeRunId: runId, lastError: null, updatedAt: 10 }
      })
    ).toThrow(/status/i)
  })

  it('continues to reject unknown fields at the shared Host boundary', () => {
    expect(() =>
      parseAgentObserverEventEnvelope(
        envelope({ type: 'started', runId, toolDefinitions: [], forgedAuthority: true } as never)
      )
    ).toThrow(/forgedAuthority|unexpected/i)
  })
})
