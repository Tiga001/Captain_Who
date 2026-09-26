import { describe, expect, it } from 'vitest'
import {
  parseAgentActionExecutionOutputForHost,
  parseAgentBuiltinCapabilityActivationApproval,
  parseAgentBuiltinCapabilityActivationProposedAction,
  parseAgentEventForHost,
  parseAgentToolIdentityForHost,
  parsePendingAgentActionSnapshotsForHost
} from '../index'
import { createAgentMcpFixtures } from './fixtures'

const { serverId, actionId, callId, builtinCapabilityApproval } = createAgentMcpFixtures()

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
