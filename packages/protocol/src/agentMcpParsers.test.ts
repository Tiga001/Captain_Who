import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  parseAgentActionExecutionOutputForHost,
  parseAgentBrowserRiskApproval,
  parseAgentBrowserRiskProposedAction,
  parseAgentBuiltinCapabilityActivationApproval,
  parseAgentBuiltinCapabilityActivationProposedAction,
  parseAgentBuiltinMcpToolApproval,
  parseAgentBuiltinMcpToolApprovalProposedAction,
  parseAgentEventForHost,
  parseAgentMcpToolApproval,
  parseAgentMcpToolInvocationEvent,
  parseAgentToolIdentityForHost,
  parsePendingAgentActionSnapshotsForHost
} from './agentMcpParsers'

const serverId = 'ce18d23c-e74f-4e89-8695-ce1e7c60ec92'
const actionId = '94c2f39c-ddaa-49bb-a3ef-8756053d68c8'
const invocationId = 'a8a6102c-8ad6-45d5-bb0d-3e4f0ad2a30f'
const callId = `tc1_${'a'.repeat(43)}`
const capabilityActivationId = '67b4ea45-d0e2-42d0-aee4-6da42d5e45a7'
const browserRiskApprovalId = '2e976cb0-0b1e-40ab-8aa8-84ff41103d2f'
const builtinMcpApprovalId = '4c6cc4a7-c8eb-4583-b7c5-dc7d2bf86293'
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
    approvalStatus: 'required',
    reason: null
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

const builtinCapabilityApproval = {
  actionId,
  activationId: capabilityActivationId,
  runId: 'run-owned',
  callId,
  capabilityId: 'browser_automation',
  displayName: 'Browser automation',
  reason: 'Open the requested page in the in-app browser.',
  manifestDigest: `sha256:${'e'.repeat(64)}`,
  policyRevision: 7,
  createdAt: 1_753_843_200,
  expiresAt: 1_753_844_100,
  approvalStatus: 'required'
} as const

const browserRiskApproval = {
  schemaVersion: 1,
  actionId,
  riskApprovalId: browserRiskApprovalId,
  runId: 'run-owned',
  callId,
  capabilityId: 'browser_automation',
  capabilityActivationId,
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
  manifestDigest: `sha256:${'e'.repeat(64)}`,
  policyRevision: 7,
  createdAt: 1_753_843_200,
  expiresAt: 1_753_844_100,
  approvalStatus: 'required'
} as const

const builtinMcpToolApproval = {
  schemaVersion: 1,
  identity: {
    actionId,
    approvalId: builtinMcpApprovalId,
    runId: 'run-owned',
    callId,
    capabilityId: 'browser_automation',
    capabilityActivationId,
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
  callReason: 'Set the fixture editor value.',
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
  outcome: null,
  isError: null,
  errorCode: null,
  durationMs: null,
  diagnostics: null,
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

describe('built-in capability Host-boundary contract', () => {
  it('strictly parses the frozen activation approval and approval event', () => {
    expect(parseAgentBuiltinCapabilityActivationApproval(builtinCapabilityApproval)).toEqual(
      builtinCapabilityApproval
    )
    expect(
      parseAgentBuiltinCapabilityActivationProposedAction({
        type: 'builtin_capability_activation',
        approval: builtinCapabilityApproval
      })
    ).toEqual({
      type: 'builtin_capability_activation',
      approval: builtinCapabilityApproval
    })
    expect(
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-owned',
        action: {
          type: 'builtin_capability_activation',
          approval: builtinCapabilityApproval
        }
      })
    ).toMatchObject({
      type: 'approval_required',
      action: { type: 'builtin_capability_activation' }
    })
  })

  it('rejects extra authority fields, invalid identities, cross-run events and invalid expiry', () => {
    expect(() =>
      parseAgentBuiltinCapabilityActivationApproval({
        ...builtinCapabilityApproval,
        executable: '/tmp/managed-browser-server'
      })
    ).toThrow(/unexpected field/)
    expect(() =>
      parseAgentBuiltinCapabilityActivationApproval({
        ...builtinCapabilityApproval,
        capabilityId: 'browser.automation'
      })
    ).toThrow(/unexpected value browser\.automation/)
    expect(() =>
      parseAgentBuiltinCapabilityActivationApproval({
        ...builtinCapabilityApproval,
        capabilityId: 'Browser Automation'
      })
    ).toThrow(/unexpected value Browser Automation/)
    expect(() =>
      parseAgentBuiltinCapabilityActivationApproval({
        ...builtinCapabilityApproval,
        displayName: '   '
      })
    ).toThrow(/displayName must not be blank/)
    expect(() =>
      parseAgentBuiltinCapabilityActivationApproval({
        ...builtinCapabilityApproval,
        manifestDigest: 'e'.repeat(64)
      })
    ).toThrow(/sha256:-prefixed/)
    expect(() =>
      parseAgentBuiltinCapabilityActivationApproval({
        ...builtinCapabilityApproval,
        expiresAt: builtinCapabilityApproval.createdAt + 899
      })
    ).toThrow(/15 minute/)
    expect(() =>
      parseAgentBuiltinCapabilityActivationApproval({
        ...builtinCapabilityApproval,
        expiresAt: builtinCapabilityApproval.createdAt + 901
      })
    ).toThrow(/15 minute/)
    expect(() =>
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-other',
        action: {
          type: 'builtin_capability_activation',
          approval: builtinCapabilityApproval
        }
      })
    ).toThrow(/runId must match/)
    expect(() =>
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-owned',
        action: {
          type: 'builtin_capability_activation',
          approval: { ...builtinCapabilityApproval, approvalStatus: 'approved' }
        }
      })
    ).toThrow(/must remain required/)
  })

  it('strictly parses built-in capability Tool identity without Server configuration', () => {
    const identity = {
      type: 'builtin_capability',
      capabilityId: 'browser_automation',
      managedMcpId: 'builtin.browser_automation.mcp',
      packageName: '@playwright/mcp',
      packageVersion: '0.0.79',
      upstreamCatalogDigest: `sha256:${'1'.repeat(64)}`,
      policyDigest: `sha256:${'2'.repeat(64)}`,
      manifestDigest: `sha256:${'f'.repeat(64)}`,
      toolId: 'browser.navigate',
      rawName: 'browser_navigate',
      modelName: 'browser_navigate',
      upstreamSchemaDigest: `sha256:${'3'.repeat(64)}`,
      hostOverlayDigest: `sha256:${'4'.repeat(64)}`,
      hostInputSchemaDigest: `sha256:${'5'.repeat(64)}`
    }
    expect(parseAgentToolIdentityForHost(identity)).toEqual(identity)
    expect(() =>
      parseAgentToolIdentityForHost({ ...identity, capabilityId: 'browser.automation' })
    ).toThrow(/unexpected value browser\.automation/)
    expect(() => parseAgentToolIdentityForHost({ ...identity, serverId })).toThrow(
      /unexpected field/
    )
    expect(() =>
      parseAgentToolIdentityForHost({
        type: identity.type,
        capabilityId: identity.capabilityId,
        managedMcpId: identity.managedMcpId,
        packageName: identity.packageName,
        packageVersion: identity.packageVersion,
        upstreamCatalogDigest: identity.upstreamCatalogDigest,
        policyDigest: identity.policyDigest,
        manifestDigest: identity.manifestDigest,
        toolId: identity.toolId,
        rawName: identity.rawName,
        upstreamSchemaDigest: identity.upstreamSchemaDigest,
        hostOverlayDigest: identity.hostOverlayDigest,
        hostInputSchemaDigest: identity.hostInputSchemaDigest
      })
    ).toThrow(/modelName/)
    expect(() =>
      parseAgentToolIdentityForHost({ ...identity, packageName: ' @playwright/mcp' })
    ).toThrow(/packageName/)
    expect(() =>
      parseAgentToolIdentityForHost({ ...identity, upstreamCatalogDigest: 'a'.repeat(64) })
    ).toThrow(/upstreamCatalogDigest/)
    expect(() =>
      parseAgentToolIdentityForHost({ ...identity, rawName: 'Browser.Navigate' })
    ).toThrow(/rawName/)
    expect(() =>
      parseAgentToolIdentityForHost({ ...identity, modelName: 'browser.navigate' })
    ).toThrow(/Provider-visible Tool name/)
    expect(() => parseAgentToolIdentityForHost({ ...identity, type: 'future_identity' })).toThrow(
      /supported Tool identity/
    )
  })

  it('strictly parses pending and execution projections for capability activation', () => {
    const action = {
      type: 'builtin_capability_activation',
      approval: builtinCapabilityApproval
    }
    const pending = {
      actionId,
      actionType: 'builtin_capability_activation',
      toolName: 'activate_capability',
      toolCallId: callId,
      runId: 'run-owned',
      conversationId: 'conversation-owned',
      assistantMessageId: 'assistant-owned',
      action,
      createdAt: 1_753_843_200_000,
      status: 'pending'
    }
    expect(parsePendingAgentActionSnapshotsForHost([pending])).toEqual([pending])
    for (const status of [
      'approved',
      'executing',
      'rejected',
      'cancelled',
      'completed',
      'failed'
    ]) {
      expect(() => parsePendingAgentActionSnapshotsForHost([{ ...pending, status }])).toThrow(
        /expected pending/
      )
    }
    expect(() =>
      parsePendingAgentActionSnapshotsForHost([
        {
          ...pending,
          action: {
            ...action,
            approval: { ...builtinCapabilityApproval, approvalStatus: 'approved' }
          }
        }
      ])
    ).toThrow(/must remain required/)
    expect(() =>
      parsePendingAgentActionSnapshotsForHost([{ ...pending, toolCallId: null }])
    ).toThrow(/expected a string/)
    expect(() =>
      parsePendingAgentActionSnapshotsForHost([{ ...pending, manifest: { tools: [] } }])
    ).toThrow(/unexpected field/)

    const output = parseAgentActionExecutionOutputForHost({
      actionId,
      actionType: 'builtin_capability_activation',
      toolName: 'activate_capability',
      status: 'approved',
      toolResult: { managedServerConfig: 'must-not-reach-renderer' },
      agentOutput: {
        content: 'Capability activated.',
        status: 'completed',
        runId: 'run-owned',
        events: [],
        toolDefinitions: [{ description: 'must-not-reach-renderer' }],
        proposedActions: []
      }
    })
    expect(output.toolResult).toBeUndefined()
    expect(output.agentOutput.toolDefinitions).toEqual([])
    expect(JSON.stringify(output)).not.toContain('managedServerConfig')
  })
})

describe('built-in MCP Tool approval Host-boundary contract', () => {
  it('strictly parses the safe approval, event, pending snapshot and execution receipt', () => {
    expect(parseAgentBuiltinMcpToolApproval(builtinMcpToolApproval)).toEqual(builtinMcpToolApproval)
    const action = {
      type: 'builtin_mcp_tool_approval',
      approval: builtinMcpToolApproval
    } as const
    expect(parseAgentBuiltinMcpToolApprovalProposedAction(action)).toEqual(action)
    expect(
      parseAgentEventForHost({ type: 'approval_required', runId: 'run-owned', action })
    ).toEqual({ type: 'approval_required', runId: 'run-owned', action })

    const pending = {
      actionId,
      actionType: 'builtin_mcp_tool_approval',
      toolName: 'browser_evaluate',
      toolCallId: callId,
      runId: 'run-owned',
      conversationId: 'conversation-owned',
      assistantMessageId: 'assistant-owned',
      action,
      createdAt: 1_753_843_200_000,
      status: 'pending'
    }
    expect(parsePendingAgentActionSnapshotsForHost([pending])).toEqual([pending])

    const output = parseAgentActionExecutionOutputForHost({
      actionId,
      actionType: 'builtin_mcp_tool_approval',
      toolName: 'browser_evaluate',
      status: 'rejected',
      toolResult: {
        cookie: 'PRIVATE_COOKIE_CANARY',
        storage: 'PRIVATE_STORAGE_CANARY',
        code: 'PRIVATE_SCRIPT_CANARY'
      },
      agentOutput: {
        content: 'The user declined this sensitive browser operation.',
        status: 'completed',
        runId: 'run-owned',
        events: [],
        toolDefinitions: [{ description: 'must-not-reach-renderer' }],
        proposedActions: []
      }
    })
    expect(output.toolResult).toBeUndefined()
    expect(output.agentOutput.toolDefinitions).toEqual([])
    expect(JSON.stringify(output)).not.toContain('PRIVATE_')
  })

  it('rejects hidden authority, identity drift, raw paths and non-canonical risks', () => {
    expect(() =>
      parseAgentBuiltinMcpToolApproval({
        ...builtinMcpToolApproval,
        headers: { authorization: 'PRIVATE_AUTH_CANARY' }
      })
    ).toThrow(/unexpected field/)
    expect(() =>
      parseAgentBuiltinMcpToolApproval({
        ...builtinMcpToolApproval,
        identity: { ...builtinMcpToolApproval.identity, modelName: 'browser_click' }
      })
    ).toThrow(/identities must match/)
    expect(() =>
      parseAgentBuiltinMcpToolApproval({
        ...builtinMcpToolApproval,
        resourceSummary: {
          ...builtinMcpToolApproval.resourceSummary,
          fileBasenames: ['/Users/private/secret.txt']
        }
      })
    ).toThrow(/must remain a basename/)
    expect(() =>
      parseAgentBuiltinMcpToolApproval({
        ...builtinMcpToolApproval,
        riskKinds: ['page_script_execution', 'page_script_execution']
      })
    ).toThrow(/unique and canonical/)
    expect(() =>
      parseAgentBuiltinMcpToolApproval({
        ...builtinMcpToolApproval,
        identity: { ...builtinMcpToolApproval.identity, origin: 'https://fixture.example/path' }
      })
    ).toThrow(/origin/)
  })

  it('fails closed for mismatched or non-pending hydration identities', () => {
    const action = {
      type: 'builtin_mcp_tool_approval',
      approval: builtinMcpToolApproval
    } as const
    const pending = {
      actionId,
      actionType: 'builtin_mcp_tool_approval',
      toolName: 'browser_evaluate',
      toolCallId: callId,
      runId: 'run-owned',
      conversationId: null,
      assistantMessageId: null,
      action,
      createdAt: 1_753_843_200_000,
      status: 'pending'
    }
    expect(() =>
      parsePendingAgentActionSnapshotsForHost([{ ...pending, status: 'approved' }])
    ).toThrow(/expected pending/)
    expect(() =>
      parsePendingAgentActionSnapshotsForHost([{ ...pending, toolName: 'browser_cookie_get' }])
    ).toThrow(/expected browser_evaluate/)
    expect(() =>
      parsePendingAgentActionSnapshotsForHost([{ ...pending, toolCallId: null }])
    ).toThrow(/expected a string/)
    expect(() =>
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-other',
        action
      })
    ).toThrow(/runId must match/)
  })
})

describe('browser risk Host-boundary contract', () => {
  it('strictly parses the exact approval, event, pending snapshot and execution projection', () => {
    expect(parseAgentBrowserRiskApproval(browserRiskApproval)).toEqual(browserRiskApproval)
    const action = {
      type: 'browser_risk_approval',
      approval: browserRiskApproval
    } as const
    expect(parseAgentBrowserRiskProposedAction(action)).toEqual(action)
    expect(
      parseAgentEventForHost({ type: 'approval_required', runId: 'run-owned', action })
    ).toEqual({ type: 'approval_required', runId: 'run-owned', action })

    const pending = {
      actionId,
      actionType: 'browser_risk_approval',
      toolName: 'browser_navigate',
      toolCallId: callId,
      runId: 'run-owned',
      conversationId: 'conversation-owned',
      assistantMessageId: 'assistant-owned',
      action,
      createdAt: 1_753_843_200_000,
      status: 'pending'
    }
    expect(parsePendingAgentActionSnapshotsForHost([pending])).toEqual([pending])

    const output = parseAgentActionExecutionOutputForHost({
      actionId,
      actionType: 'browser_risk_approval',
      toolName: 'browser_navigate',
      status: 'rejected',
      toolResult: { cookie: 'PRIVATE_COOKIE_CANARY' },
      agentOutput: {
        content: 'The user declined this browser destination.',
        status: 'completed',
        runId: 'run-owned',
        events: [],
        toolDefinitions: [{ description: 'must-not-reach-renderer' }],
        proposedActions: []
      }
    })
    expect(output.toolResult).toBeUndefined()
    expect(output.agentOutput.toolDefinitions).toEqual([])
    expect(JSON.stringify(output)).not.toContain('PRIVATE_COOKIE_CANARY')
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        actionId,
        actionType: 'browser_risk_approval',
        toolName: 'browser_run_code_unsafe',
        status: 'rejected',
        agentOutput: {
          content: '',
          status: 'completed',
          runId: 'run-owned',
          events: [],
          toolDefinitions: [],
          proposedActions: []
        }
      })
    ).toThrow()
  })

  it('rejects hidden authority, inconsistent destinations and malformed frozen identity', () => {
    expect(() =>
      parseAgentBrowserRiskApproval({ ...browserRiskApproval, headers: { authorization: 'x' } })
    ).toThrow(/unexpected field/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        destination: {
          ...browserRiskApproval.destination,
          resolutionFingerprint: `sha256:${'f'.repeat(64)}`
        }
      })
    ).toThrow(/unexpected field/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        destination: {
          ...browserRiskApproval.destination,
          normalizedUrl: 'http://user:secret@127.0.0.1:3000/dashboard'
        }
      })
    ).toThrow(/inconsistent/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        destination: {
          ...browserRiskApproval.destination,
          normalizedUrl: 'http://127.0.0.1:3000/dashboard?token=secret'
        }
      })
    ).toThrow(/inconsistent/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        destination: { ...browserRiskApproval.destination, effectivePort: 3001 }
      })
    ).toThrow(/inconsistent/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        riskKinds: ['loopback', 'loopback']
      })
    ).toThrow(/duplicates/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        expiresAt: browserRiskApproval.createdAt + 899
      })
    ).toThrow(/15 minute/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        createdAt: 253_402_300_800,
        expiresAt: 253_402_301_700
      })
    ).toThrow(/date range/)
    expect(() =>
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-other',
        action: { type: 'browser_risk_approval', approval: browserRiskApproval }
      })
    ).toThrow(/runId must match/)
    expect(() =>
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-owned',
        action: {
          type: 'browser_risk_approval',
          approval: { ...browserRiskApproval, approvalStatus: 'approved' }
        }
      })
    ).toThrow(/must remain required/)
  })
})

describe('durable presentation trace ordering contract', () => {
  it('accepts only the exact built-in capability Tool identity projection', () => {
    const event = {
      type: 'tool_call',
      runId: 'run-browser-capability',
      traceSequence: 7,
      identity: {
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
      call: {
        id: callId,
        tool: 'browser_navigate',
        args: { url: 'https://user:PRIVATE_PASSWORD@example.com/' },
        approvalStatus: 'not_required',
        reason: null
      }
    }

    expect(parseAgentEventForHost(event)).toEqual({
      ...event,
      call: { ...event.call, args: {} }
    })
    expect(JSON.stringify(parseAgentEventForHost(event))).not.toContain('PRIVATE_PASSWORD')
    expect(() =>
      parseAgentEventForHost({
        ...event,
        identity: { ...event.identity, cdpEndpoint: 'PRIVATE_CDP_CANARY' }
      })
    ).toThrow(/unexpected field/)
    expect(() =>
      parseAgentEventForHost({
        ...event,
        call: { ...event.call, tool: 'browser_snapshot' }
      })
    ).toThrow(/identity must match call\.tool/)
  })

  it('strictly parses required Tool, narration, compaction, and Runtime error sequence fields', () => {
    const toolCall = {
      type: 'tool_call',
      runId: 'run-trace',
      traceSequence: 4,
      identity: { type: 'builtin', toolName: 'read_file' },
      call: {
        id: callId,
        tool: 'read_file',
        args: { path: 'README.md' },
        approvalStatus: 'not_required',
        reason: null
      }
    }
    expect(parseAgentEventForHost(toolCall)).toEqual(toolCall)
    const missingIdentity = { ...toolCall } as Record<string, unknown>
    delete missingIdentity.identity
    expect(() => parseAgentEventForHost(missingIdentity)).toThrow(/identity/)
    expect(() =>
      parseAgentEventForHost({
        ...toolCall,
        identity: { type: 'builtin', toolName: 'different_tool' }
      })
    ).toThrow(/identity must match call\.tool/)
    expect(
      parseAgentEventForHost({
        type: 'message_stream_committed',
        runId: 'run-trace',
        streamId: 'stream-trace',
        traceSequence: null
      })
    ).toMatchObject({ type: 'message_stream_committed', traceSequence: null })
    expect(
      parseAgentEventForHost({
        type: 'context_compaction_started',
        runId: 'run-trace',
        operationId: 'compact-1',
        traceSequence: 5
      })
    ).toMatchObject({ type: 'context_compaction_started', traceSequence: 5 })
    expect(
      parseAgentEventForHost({
        type: 'context_compaction_finished',
        runId: 'run-trace',
        operationId: 'compact-1',
        outcome: 'applied',
        traceSequence: 5
      })
    ).toMatchObject({ type: 'context_compaction_finished', traceSequence: 5 })
    expect(
      parseAgentEventForHost({
        type: 'error',
        runId: 'run-trace',
        traceSequence: 6,
        message: 'stopped',
        recoverable: false
      })
    ).toMatchObject({ type: 'error', traceSequence: 6 })

    for (const event of [
      toolCall,
      {
        type: 'message_stream_committed',
        runId: 'run-trace',
        streamId: 'stream-trace',
        traceSequence: 3
      },
      {
        type: 'context_compaction_started',
        runId: 'run-trace',
        operationId: 'compact-1',
        traceSequence: 5
      },
      {
        type: 'context_compaction_finished',
        runId: 'run-trace',
        operationId: 'compact-1',
        outcome: 'applied',
        traceSequence: 5
      },
      {
        type: 'error',
        runId: 'run-trace',
        traceSequence: null,
        message: 'stopped',
        recoverable: false
      }
    ]) {
      const missing = { ...event } as Record<string, unknown>
      delete missing.traceSequence
      expect(() => parseAgentEventForHost(missing)).toThrow(/traceSequence/)
    }
  })
})

describe('current LLM retry Host contract', () => {
  it('keeps the complete structured retry metadata', () => {
    const parsed = parseAgentEventForHost({
      type: 'llm_retry',
      runId: 'run-retry',
      streamId: 'stream-1',
      category: 'rate_limited',
      providerCode: 'rate_limit_exceeded',
      delayMs: 5_000,
      retryAt: 1_800_000_005_000,
      attempt: 2,
      maxAttempts: 3
    })

    expect(parsed).toEqual({
      type: 'llm_retry',
      runId: 'run-retry',
      streamId: 'stream-1',
      category: 'rate_limited',
      providerCode: 'rate_limit_exceeded',
      delayMs: 5_000,
      retryAt: 1_800_000_005_000,
      attempt: 2,
      maxAttempts: 3
    })
  })

  it('rejects the removed reason shape, missing scheduling fields, and unknown categories', () => {
    const current = {
      type: 'llm_retry',
      runId: 'run-retry',
      streamId: 'stream-current',
      category: 'network',
      delayMs: 1_000,
      retryAt: 1_800_000_001_000,
      attempt: 2,
      maxAttempts: 3
    }
    expect(() =>
      parseAgentEventForHost({ ...current, reason: 'removed provider-authored text' })
    ).toThrow(/unexpected field reason/)
    expect(() => {
      const missingDelay: Record<string, unknown> = { ...current }
      delete missingDelay.delayMs
      parseAgentEventForHost(missingDelay)
    }).toThrow(/delayMs/)
    expect(() =>
      parseAgentEventForHost({
        ...current,
        type: 'llm_retry',
        streamId: 'stream-future',
        category: 'new_provider_category'
      })
    ).toThrow(/supported retry category/)
  })

  it('replaces stream reset diagnostics with a stable lifecycle reason', () => {
    expect(
      parseAgentEventForHost({
        type: 'message_stream_reset',
        runId: 'run-retry',
        streamId: 'stream-1',
        reason: 'secret upstream response body'
      })
    ).toEqual({
      type: 'message_stream_reset',
      runId: 'run-retry',
      streamId: 'stream-1',
      reason: 'retrying_model_request'
    })
  })

  it('bounds retry scheduling metadata and requires canonical provider codes', () => {
    const base = {
      type: 'llm_retry',
      runId: 'run-retry',
      streamId: 'stream-1',
      category: 'network',
      delayMs: 1_000,
      retryAt: 1_800_000_001_000,
      attempt: 2,
      maxAttempts: 6
    }
    expect(() => parseAgentEventForHost({ ...base, delayMs: 60_001 })).toThrow(/delayMs/)
    expect(() => parseAgentEventForHost({ ...base, maxAttempts: 7 })).toThrow(/must not exceed 6/)
    expect(() =>
      parseAgentEventForHost({ ...base, providerCode: 'Provider response body' })
    ).toThrow(/machine-readable code/)
  })
})

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
    const callWithoutReason: Record<string, unknown> = { ...approval.call }
    delete callWithoutReason.reason
    expect(() => parseAgentMcpToolApproval({ ...approval, call: callWithoutReason })).toThrow(
      /reason is required/
    )
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

  it('requires the current diagnostics field even when its value is null', () => {
    const withoutDiagnostics: Record<string, unknown> = { ...invocation }
    delete withoutDiagnostics.diagnostics
    expect(() => parseAgentMcpToolInvocationEvent(withoutDiagnostics)).toThrow(
      /diagnostics is required/
    )
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

  it('requires the current lifecycle reason field and rejects an oversized one', () => {
    const withoutDisplayReason: Record<string, unknown> = { ...invocation }
    delete withoutDisplayReason.displayReason
    expect(() => parseAgentMcpToolInvocationEvent(withoutDisplayReason)).toThrow(
      /displayReason is required/
    )
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
