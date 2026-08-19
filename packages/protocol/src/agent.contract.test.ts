import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  AGENT_APPROVE_ACTION_METHOD,
  AGENT_CANCEL_ACTION_METHOD,
  AGENT_CANCEL_RUN_METHOD,
  AGENT_COMMAND_SESSIONS_GET_METHOD,
  AGENT_COMMAND_SESSIONS_LIST_METHOD,
  AGENT_CLEAR_USAGE_RECORDS_METHOD,
  AGENT_EVENT_NOTIFICATION_METHOD,
  AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD,
  AGENT_GET_FILE_WRITE_DIFF_METHOD,
  AGENT_GET_USAGE_SUMMARY_METHOD,
  AGENT_LIST_PENDING_ACTIONS_METHOD,
  AGENT_READ_FILE_DRAFT_METHOD,
  AGENT_REJECT_ACTION_METHOD,
  AGENT_REWRITE_CONVERSATION_TURN_METHOD,
  AGENT_START_CONVERSATION_TURN_METHOD,
  AGENT_STEER_RUN_METHOD,
  type AgentEvent,
  type AgentToolIdentity,
  type ConversationTurnTraceItem
} from './agent'
import {
  AGENT_GET_PROVIDER_TRANSITION_STATUS_METHOD,
  AGENT_PREFLIGHT_PROVIDER_TRANSITION_METHOD,
  AGENT_PROVIDER_TRANSITION_NOTIFICATION_METHOD,
  AGENT_START_PROVIDER_TRANSITION_METHOD
} from './providerTransition'

const fixture = JSON.parse(
  readFileSync(resolve(process.cwd(), 'packages/protocol/fixtures/agent-contract-v1.json'), 'utf8')
)

const events: Record<string, AgentEvent> = {
  messageDelta: {
    type: 'message_delta',
    runId: 'run-contract-v1',
    streamId: 'stream-contract-v1',
    delta: 'hello'
  },
  messageStreamCommitted: {
    type: 'message_stream_committed',
    runId: 'run-contract-v1',
    streamId: 'stream-contract-v1',
    traceSequence: 3
  },
  toolCall: {
    type: 'tool_call',
    runId: 'run-contract-v1',
    traceSequence: 4,
    identity: { type: 'builtin', toolName: 'read_file' },
    call: {
      id: 'call-contract-v1',
      tool: 'read_file',
      args: { path: 'README.md' },
      approvalStatus: 'not_required',
      reason: 'Inspect project documentation.'
    }
  },
  toolResult: {
    type: 'tool_result',
    runId: 'run-contract-v1',
    result: {
      callId: 'call-contract-v1',
      tool: 'read_file',
      ok: true,
      result: { content: 'MyCopilot Next' }
    }
  },
  commandStarted: {
    type: 'command_started',
    runId: 'run-contract-v1',
    conversationId: 'conversation-contract-v1',
    assistantMessageId: 'assistant-contract-v1',
    projectId: 'project-contract-v1',
    callId: 'call-command-contract-v1',
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    startedAt: 10
  },
  commandOutput: {
    type: 'command_output',
    runId: 'run-contract-v1',
    conversationId: 'conversation-contract-v1',
    assistantMessageId: 'assistant-contract-v1',
    projectId: 'project-contract-v1',
    callId: 'call-command-contract-v1',
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    sequence: 1,
    stream: 'stdout',
    output: 'ready\n'
  },
  commandExited: {
    type: 'command_exited',
    runId: 'run-contract-v1',
    conversationId: 'conversation-contract-v1',
    assistantMessageId: 'assistant-contract-v1',
    projectId: 'project-contract-v1',
    callId: 'call-command-contract-v1',
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    status: 'exited',
    exitCode: 0,
    endedAt: 20,
    latestSequence: 1,
    outputTruncated: false
  },
  commandInterrupted: {
    type: 'command_interrupted',
    runId: 'run-contract-v1',
    conversationId: 'conversation-contract-v1',
    assistantMessageId: 'assistant-contract-v1',
    projectId: 'project-contract-v1',
    callId: 'call-command-contract-v1',
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    endedAt: 21,
    latestSequence: 1,
    outputTruncated: false
  },
  done: {
    type: 'done',
    runId: 'run-contract-v1',
    success: true,
    status: 'completed',
    content: 'done'
  }
}

const toolIdentities = [
  { type: 'builtin', toolName: 'read_file' },
  {
    type: 'runtime_extension',
    extensionId: 'office',
    toolName: 'office_document'
  },
  {
    type: 'builtin_capability',
    capabilityId: 'browser_automation',
    managedMcpId: 'builtin.browser_automation.mcp',
    packageName: '@playwright/mcp',
    packageVersion: '0.0.79',
    upstreamCatalogDigest: `sha256:${'1'.repeat(64)}`,
    policyDigest: `sha256:${'2'.repeat(64)}`,
    manifestDigest: `sha256:${'e'.repeat(64)}`,
    toolId: 'browser.navigate',
    rawName: 'browser_navigate',
    modelName: 'browser_navigate',
    upstreamSchemaDigest: `sha256:${'3'.repeat(64)}`,
    hostOverlayDigest: `sha256:${'4'.repeat(64)}`,
    hostInputSchemaDigest: `sha256:${'5'.repeat(64)}`
  },
  {
    type: 'mcp',
    provenance: {
      serverId: '7f4a2d91-24ab-4d24-9eed-63daf26a6c15',
      scope: { type: 'project', projectId: 'project-fixture' },
      rawToolName: 'add_numbers',
      modelToolName: 'mcp__fixture_7f4a2d91__add_numbers',
      configEpoch: '11111111-1111-4111-8111-111111111111',
      registryRevision: 8,
      configDigest: 'a'.repeat(64),
      catalogGeneration: 4,
      catalogDigest: 'b'.repeat(64),
      catalogSchemaDigest: 'c'.repeat(64),
      schemaDigest: 'd'.repeat(64),
      schemaNormalizerVersion: 1
    }
  },
  { type: 'unregistered', toolName: 'hallucinated_tool' }
] satisfies AgentToolIdentity[]

const mcpTraceToolCall = {
  type: 'tool_call',
  sequence: 3,
  callId: 'call-mcp-fixture',
  tool: 'mcp__fixture_7f4a2d91__add_numbers',
  operation: { left: 2, right: 3 },
  approvalStatus: 'not_required',
  truncated: false,
  provenance: toolIdentities[3]
} satisfies ConversationTurnTraceItem

describe('Agent cross-language golden contract', () => {
  it('keeps TypeScript event discriminants and field casing aligned with the fixture', () => {
    expect(events).toEqual(fixture.events)
  })

  it('keeps the JSON-RPC method namespace centralized', () => {
    expect(fixture.methods).toEqual({
      cancelRun: AGENT_CANCEL_RUN_METHOD,
      steerRun: AGENT_STEER_RUN_METHOD,
      startConversationTurn: AGENT_START_CONVERSATION_TURN_METHOD,
      rewriteConversationTurn: AGENT_REWRITE_CONVERSATION_TURN_METHOD,
      preflightProviderTransition: AGENT_PREFLIGHT_PROVIDER_TRANSITION_METHOD,
      startProviderTransition: AGENT_START_PROVIDER_TRANSITION_METHOD,
      getProviderTransitionStatus: AGENT_GET_PROVIDER_TRANSITION_STATUS_METHOD,
      providerTransitionNotification: AGENT_PROVIDER_TRANSITION_NOTIFICATION_METHOD,
      getContextWindowSnapshot: AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD,
      listCommandSessions: AGENT_COMMAND_SESSIONS_LIST_METHOD,
      getCommandSession: AGENT_COMMAND_SESSIONS_GET_METHOD,
      listPendingActions: AGENT_LIST_PENDING_ACTIONS_METHOD,
      approveAction: AGENT_APPROVE_ACTION_METHOD,
      rejectAction: AGENT_REJECT_ACTION_METHOD,
      cancelAction: AGENT_CANCEL_ACTION_METHOD,
      getUsageSummary: AGENT_GET_USAGE_SUMMARY_METHOD,
      clearUsageRecords: AGENT_CLEAR_USAGE_RECORDS_METHOD,
      readFileDraft: AGENT_READ_FILE_DRAFT_METHOD,
      getFileWriteDiff: AGENT_GET_FILE_WRITE_DIFF_METHOD,
      eventNotification: AGENT_EVENT_NOTIFICATION_METHOD
    })
  })
})

describe('Agent tool identity contract', () => {
  it('uses the Rust-compatible tagged identity and MCP scope field casing', () => {
    expect(toolIdentities).toEqual([
      { type: 'builtin', toolName: 'read_file' },
      {
        type: 'runtime_extension',
        extensionId: 'office',
        toolName: 'office_document'
      },
      {
        type: 'builtin_capability',
        capabilityId: 'browser_automation',
        managedMcpId: 'builtin.browser_automation.mcp',
        packageName: '@playwright/mcp',
        packageVersion: '0.0.79',
        upstreamCatalogDigest: `sha256:${'1'.repeat(64)}`,
        policyDigest: `sha256:${'2'.repeat(64)}`,
        manifestDigest: `sha256:${'e'.repeat(64)}`,
        toolId: 'browser.navigate',
        rawName: 'browser_navigate',
        modelName: 'browser_navigate',
        upstreamSchemaDigest: `sha256:${'3'.repeat(64)}`,
        hostOverlayDigest: `sha256:${'4'.repeat(64)}`,
        hostInputSchemaDigest: `sha256:${'5'.repeat(64)}`
      },
      {
        type: 'mcp',
        provenance: {
          serverId: '7f4a2d91-24ab-4d24-9eed-63daf26a6c15',
          scope: { type: 'project', projectId: 'project-fixture' },
          rawToolName: 'add_numbers',
          modelToolName: 'mcp__fixture_7f4a2d91__add_numbers',
          configEpoch: '11111111-1111-4111-8111-111111111111',
          registryRevision: 8,
          configDigest: 'a'.repeat(64),
          catalogGeneration: 4,
          catalogDigest: 'b'.repeat(64),
          catalogSchemaDigest: 'c'.repeat(64),
          schemaDigest: 'd'.repeat(64),
          schemaNormalizerVersion: 1
        }
      },
      { type: 'unregistered', toolName: 'hallucinated_tool' }
    ])
  })

  it('requires typed provenance on a tool-call trace item', () => {
    expect(mcpTraceToolCall.provenance).toEqual(toolIdentities[3])
  })
})
