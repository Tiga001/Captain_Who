import { describe, expect, it } from 'vitest'

import {
  MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
  parseBrowserRiskAuthorizeInput,
  parseBrowserRiskAuthorizeOutput,
  parseManagedPlaywrightCommandNotification,
  parseManagedPlaywrightCompletionInput,
  parseManagedPlaywrightDispatchPhaseInput,
  parseManagedPlaywrightDispatchPhaseOutput
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
  it('preserves a queue timeout as a distinct, undispatched completion', () => {
    const input = {
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
      requestId: REQUEST_ID,
      outcome: {
        type: 'error',
        code: 'queue_timeout',
        dispatchCertainty: 'definitely_not_dispatched'
      }
    }
    expect(parseManagedPlaywrightCompletionInput(input)).toEqual(input)
  })

  it('strictly validates monotonic dispatch phase acknowledgements', () => {
    expect(MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION).toBe(4)
    const input = {
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
      requestId: REQUEST_ID,
      phase: 'possibly_dispatched' as const
    }
    expect(parseManagedPlaywrightDispatchPhaseInput(input)).toEqual(input)
    expect(
      parseManagedPlaywrightDispatchPhaseOutput({
        schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
        accepted: true
      })
    ).toEqual({
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
      accepted: true
    })
    expect(() =>
      parseManagedPlaywrightDispatchPhaseInput({ ...input, phase: 'request_queued' })
    ).toThrow()
    expect(() =>
      parseManagedPlaywrightDispatchPhaseInput({
        ...input,
        dispatchCertainty: 'possibly_dispatched'
      })
    ).toThrow()
  })

  it('accepts only the Rust call_tool camelCase field projection', () => {
    const notification = {
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
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

  it('strictly validates the post-approval sensitive Tool grant without accepting secrets', () => {
    const grant = {
      grantId: 'fe9c2750-fddf-4d3e-961f-55f6c0e17395',
      approvalId: 'ed14d2ba-9dc6-40e0-bfc7-22e71cad7fb8',
      argumentsDigest: `sha256:${'b'.repeat(64)}`,
      resourceScopeDigest: `sha256:${'c'.repeat(64)}`,
      targetBindingId: '398a919a-7b03-4234-b82c-a84498cf18ac',
      targetBindingDigest: `sha256:${'d'.repeat(64)}`,
      origin: 'https://mail.example.test',
      riskKinds: ['page_script_execution'],
      expiresAtMs: 1_999_999_000_000
    }
    const notification = {
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
      requestId: REQUEST_ID,
      serverId: SERVER_ID,
      deadlineMs: 2_000_000_000_000,
      command: {
        type: 'call_tool',
        name: 'browser_evaluate',
        arguments: {
          function: '() => document.title',
          call_reason: 'Read the current page title.'
        },
        timeoutMs: 60_000,
        authorizationContext: { ...AUTHORIZATION_CONTEXT, builtinToolGrant: grant }
      }
    }
    expect(parseManagedPlaywrightCommandNotification(notification)).toEqual(notification)
    expect(() =>
      parseManagedPlaywrightCommandNotification({
        ...notification,
        command: {
          ...notification.command,
          authorizationContext: {
            ...AUTHORIZATION_CONTEXT,
            builtinToolGrant: { ...grant, cookie: 'secret-canary' }
          }
        }
      })
    ).toThrow()
    expect(() =>
      parseManagedPlaywrightCommandNotification({
        ...notification,
        command: {
          ...notification.command,
          authorizationContext: {
            ...AUTHORIZATION_CONTEXT,
            builtinToolGrant: { ...grant, origin: 'https://mail.example.test/path' }
          }
        }
      })
    ).toThrow()
    expect(() =>
      parseManagedPlaywrightCommandNotification({
        ...notification,
        command: {
          ...notification.command,
          authorizationContext: {
            ...AUTHORIZATION_CONTEXT,
            builtinToolGrant: { ...grant, riskKinds: ['page_script_execution', 'cookie_read'] }
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

  it('keeps host Artifact publish paths off the MCP tools/call result object', () => {
    const completion = {
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
      requestId: REQUEST_ID,
      outcome: {
        type: 'tool_called' as const,
        result: { content: [], isError: false },
        hostArtifactPublishPath: '/private/browser-automation-artifacts/objects/abc'
      }
    }
    expect(parseManagedPlaywrightCompletionInput(completion)).toEqual(completion)
    expect(
      parseManagedPlaywrightCompletionInput({
        schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
        requestId: REQUEST_ID,
        outcome: {
          type: 'tool_called',
          result: { content: [], isError: false }
        }
      })
    ).toEqual({
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
      requestId: REQUEST_ID,
      outcome: {
        type: 'tool_called',
        result: { content: [], isError: false }
      }
    })
    expect(() =>
      parseManagedPlaywrightCompletionInput({
        ...completion,
        outcome: {
          type: 'tool_called',
          result: { content: [], isError: false },
          hostArtifactPublishPath: ''
        }
      })
    ).toThrow()
  })

  it('accepts only the TypeScript error completion camelCase field projection', () => {
    const completion = {
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
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

  it('strictly validates proposal-time sensitive target prepare and release commands', () => {
    const prepare = {
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
      requestId: REQUEST_ID,
      serverId: SERVER_ID,
      deadlineMs: 2_000_000_000_000,
      command: {
        type: 'prepare_sensitive_tool',
        input: {
          bindingRequestId: '7c71eead-3b07-4700-8378-c61d7ea7ac68',
          bindingScope: 'managed_surface',
          runId: 'run-1',
          capabilityId: 'browser_automation',
          activationId: AUTHORIZATION_CONTEXT.activationId,
          manifestDigest: AUTHORIZATION_CONTEXT.manifestDigest,
          policyRevision: 1,
          grantExpiresAtMs: 2_000_000_000_000,
          callId: 'call-1',
          toolName: 'browser_evaluate',
          argumentsDigest: `sha256:${'b'.repeat(64)}`,
          createdAtMs: 1_999_999_000_000,
          expiresAtMs: 1_999_999_600_000,
          filePreparation: null
        }
      }
    }
    expect(parseManagedPlaywrightCommandNotification(prepare)).toEqual(prepare)
    const filePrepare = {
      ...prepare,
      command: {
        ...prepare.command,
        input: {
          ...prepare.command.input,
          toolName: 'browser_file_upload',
          filePreparation: {
            mode: 'resolved_paths',
            paths: ['/process-only/workspace/浙江大学2026年招生资料汇编.pptx']
          }
        }
      }
    }
    expect(parseManagedPlaywrightCommandNotification(filePrepare)).toEqual(filePrepare)
    expect(() =>
      parseManagedPlaywrightCommandNotification({
        ...filePrepare,
        command: {
          ...filePrepare.command,
          input: {
            ...filePrepare.command.input,
            filePreparation: { mode: 'native_picker', multiple: true }
          }
        }
      })
    ).toThrow()
    expect(() =>
      parseManagedPlaywrightCommandNotification({
        ...prepare,
        command: {
          ...prepare.command,
          input: { ...prepare.command.input, surfaceId: 'must-remain-main-only' }
        }
      })
    ).toThrow()

    const release = {
      ...prepare,
      command: {
        type: 'release_sensitive_tool_binding',
        bindingId: '398a919a-7b03-4234-b82c-a84498cf18ac',
        runId: 'run-1',
        activationId: AUTHORIZATION_CONTEXT.activationId,
        callId: 'call-1',
        reason: 'cancelled'
      }
    }
    expect(parseManagedPlaywrightCommandNotification(release)).toEqual(release)
    expect(() =>
      parseManagedPlaywrightCommandNotification({
        ...release,
        command: { ...release.command, generation: 7 }
      })
    ).toThrow()
  })

  it('strictly validates sensitive target completion outcomes', () => {
    const prepared = {
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
      requestId: REQUEST_ID,
      outcome: {
        type: 'sensitive_tool_prepared',
        bindingId: '398a919a-7b03-4234-b82c-a84498cf18ac',
        targetBindingDigest: `sha256:${'d'.repeat(64)}`,
        origin: 'https://mail.example.test',
        createdAtMs: 1_999_999_000_000,
        expiresAtMs: 1_999_999_600_000,
        fileBasenames: [],
        fileRevisionDigest: null
      }
    }
    expect(parseManagedPlaywrightCompletionInput(prepared)).toEqual(prepared)
    expect(
      parseManagedPlaywrightCompletionInput({
        ...prepared,
        outcome: { ...prepared.outcome, origin: null }
      })
    ).toEqual({ ...prepared, outcome: { ...prepared.outcome, origin: null } })
    expect(() =>
      parseManagedPlaywrightCompletionInput({
        ...prepared,
        outcome: { ...prepared.outcome, surfaceId: 'must-remain-main-only' }
      })
    ).toThrow()

    const released = {
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
      requestId: REQUEST_ID,
      outcome: { type: 'sensitive_tool_binding_released', released: true }
    }
    expect(parseManagedPlaywrightCompletionInput(released)).toEqual(released)
    expect(() =>
      parseManagedPlaywrightCompletionInput({
        ...released,
        outcome: { ...released.outcome, reason: 'cancelled' }
      })
    ).toThrow()
  })
})
