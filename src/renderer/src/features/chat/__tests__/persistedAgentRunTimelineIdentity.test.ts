import { describe, expect, it } from 'vitest'
import { parseTimelineItem } from '../../storage/persistedAgentRunTimelineValidators'

const identity = {
  type: 'builtin_capability' as const,
  capabilityId: 'browser_automation' as const,
  managedMcpId: 'builtin.browser_automation.mcp',
  packageName: '@playwright/mcp',
  packageVersion: '0.0.79',
  upstreamCatalogDigest: `sha256:${'1'.repeat(64)}`,
  policyDigest: `sha256:${'2'.repeat(64)}`,
  manifestDigest: `sha256:${'a'.repeat(64)}`,
  toolId: 'browser.navigate',
  rawName: 'browser_navigate',
  modelName: 'browser_navigate',
  upstreamSchemaDigest: `sha256:${'3'.repeat(64)}`,
  hostOverlayDigest: `sha256:${'4'.repeat(64)}`,
  hostInputSchemaDigest: `sha256:${'5'.repeat(64)}`
}

describe('persisted Tool timeline identity', () => {
  it('restores the exact typed built-in capability identity', () => {
    expect(
      parseTimelineItem({
        id: 'tool-call-browser',
        type: 'tool_call',
        callId: 'call-browser',
        traceSequence: 3,
        identity
      })
    ).toEqual({
      id: 'tool-call-browser',
      type: 'tool_call',
      callId: 'call-browser',
      traceSequence: 3,
      identity
    })
  })

  it('fails closed for malformed identity and hidden transport fields', () => {
    expect(
      parseTimelineItem({
        id: 'tool-call-browser',
        type: 'tool_call',
        callId: 'call-browser',
        identity: { ...identity, cdpEndpoint: 'PRIVATE_CDP_CANARY' }
      })
    ).toBeUndefined()
  })
})
