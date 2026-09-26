import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

// Every suite file owns fresh fixture objects; tests never share mutable contract data.
export function createAgentMcpFixtures() {
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
  return {
    serverId,
    actionId,
    invocationId,
    callId,
    capabilityActivationId,
    browserRiskApprovalId,
    builtinMcpApprovalId,
    rendererGolden,
    approval,
    builtinCapabilityApproval,
    browserRiskApproval,
    builtinMcpToolApproval,
    invocation,
    invocationDiagnostics,
    resultSizeSummary
  }
}
