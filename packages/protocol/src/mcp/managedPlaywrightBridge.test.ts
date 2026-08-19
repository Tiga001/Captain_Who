import { describe, expect, it } from 'vitest'

import {
  parseBrowserRiskAuthorizeInput,
  parseBrowserRiskAuthorizeOutput,
  parseManagedPlaywrightCommandNotification,
  parseManagedPlaywrightCompletionInput
} from './managedPlaywrightBridge'

const REQUEST_ID = '4d0dd175-0a92-4bd0-8ec4-653058561d04'
const SERVER_ID = 'b77b3d54-b7c6-4ead-9cbd-3b9fe50d5311'
const AUTHORIZATION_CONTEXT = {
  runId: 'run-1',
  capabilityId: 'browser_automation',
  activationId: '4af35bbd-cb1e-4b20-a021-92cd6b160829',
  manifestDigest: `sha256:${'a'.repeat(64)}`,
  policyRevision: 1,
  grantExpiresAtMs: 2_000_000_000_000,
  invocationId: 'c12f8536-faf9-439e-8e5a-a76049074e73',
  callId: 'call-1',
  triggerToolName: 'browser_snapshot',
  callReason: 'Inspect the local fixture.'
}

describe('managed Playwright bridge wire contract', () => {
  it('accepts only the Rust call_tool camelCase field projection', () => {
    const notification = {
      schemaVersion: 1,
      requestId: REQUEST_ID,
      serverId: SERVER_ID,
      deadlineMs: 2_000_000_000_000,
      command: {
        type: 'call_tool',
        name: 'browser_snapshot',
        arguments: { call_reason: 'Inspect the local fixture.' },
        timeoutMs: 60_000,
        authorizationContext: AUTHORIZATION_CONTEXT
      }
    }
    expect(parseManagedPlaywrightCommandNotification(notification)).toEqual(notification)
    expect(() =>
      parseManagedPlaywrightCommandNotification({
        ...notification,
        command: { ...notification.command, timeoutMs: undefined, timeout_ms: 60_000 }
      })
    ).toThrow()
    expect(() =>
      parseManagedPlaywrightCommandNotification({
        ...notification,
        command: {
          ...notification.command,
          authorizationContext: {
            ...AUTHORIZATION_CONTEXT,
            manifestDigest: 'a'.repeat(64)
          }
        }
      })
    ).toThrow()
  })

  it('strictly validates the Host-only browser risk authorization wire', () => {
    const input = {
      schemaVersion: 1,
      requestId: REQUEST_ID,
      parentRequestId: REQUEST_ID,
      authorizationContext: AUTHORIZATION_CONTEXT,
      destination: {
        normalizedUrl: 'http://127.0.0.1:3000/path',
        origin: 'http://127.0.0.1:3000',
        scheme: 'http',
        asciiHost: '127.0.0.1',
        effectivePort: 3000,
        addressClass: 'loopback',
        resolutionFingerprint: `hmac-sha256:${'b'.repeat(64)}`,
        targetFingerprint: `hmac-sha256:${'c'.repeat(64)}`
      },
      riskKinds: ['insecure_http', 'loopback'],
      trigger: 'tool_argument',
      dispatchCertainty: 'definitely_not_dispatched',
      createdAtMs: 1_000_000,
      expiresAtMs: 1_900_000
    }
    expect(parseBrowserRiskAuthorizeInput(input)).toEqual(input)
    expect(
      parseBrowserRiskAuthorizeOutput({
        schemaVersion: 1,
        decision: 'approved',
        grantId: REQUEST_ID,
        reason: null
      })
    ).toMatchObject({ decision: 'approved', grantId: REQUEST_ID })
    expect(() =>
      parseBrowserRiskAuthorizeInput({
        ...input,
        destination: { ...input.destination, query: 'secret' }
      })
    ).toThrow()
    expect(() =>
      parseBrowserRiskAuthorizeInput({
        ...input,
        destination: { ...input.destination, targetFingerprint: 'c'.repeat(64) }
      })
    ).toThrow()
  })

  it('accepts only the TypeScript error completion camelCase field projection', () => {
    const completion = {
      schemaVersion: 1,
      requestId: REQUEST_ID,
      outcome: {
        type: 'error',
        code: 'invalid_arguments',
        dispatchCertainty: 'definitely_not_dispatched'
      }
    }
    expect(parseManagedPlaywrightCompletionInput(completion)).toEqual(completion)
    expect(
      parseManagedPlaywrightCompletionInput({
        ...completion,
        outcome: {
          type: 'error',
          code: 'surface_capacity_exceeded',
          dispatchCertainty: 'definitely_not_dispatched'
        }
      })
    ).toMatchObject({ outcome: { code: 'surface_capacity_exceeded' } })
    expect(() =>
      parseManagedPlaywrightCompletionInput({
        ...completion,
        outcome: {
          type: 'error',
          code: 'invalid_arguments',
          dispatch_certainty: 'definitely_not_dispatched'
        }
      })
    ).toThrow()
    expect(() =>
      parseManagedPlaywrightCompletionInput({
        ...completion,
        outcome: {
          type: 'error',
          code: 'outcome_unknown',
          dispatchCertainty: 'response_received'
        }
      })
    ).toThrow()
  })
})
