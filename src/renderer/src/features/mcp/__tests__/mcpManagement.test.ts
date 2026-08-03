import { HostInvocationError } from '@mycopilot/host-api'
import type { TranslationKey } from '../../../config/frontendTranslations'
import { describe, expect, it } from 'vitest'
import {
  getMcpManagementErrorDetails,
  mcpOperationNeedsLaunchAuthorization,
  shouldRefreshMcpAfterError
} from '../mcpManagementErrors'
import { validateMcpDraft } from '../mcpDraftValidation'
import { toMcpPrecondition } from '../mcpManagementInputs'
import { toSafeMcpDisplayText } from '../mcpSafeDisplay'

const DIGEST = 'a'.repeat(64)
const SERVER_ID = '11111111-1111-4111-8111-111111111111'
const CONFIG_EPOCH = '22222222-2222-4222-8222-222222222222'
const translate = (key: TranslationKey) => key

describe('MCP Renderer safety helpers', () => {
  it('removes control and bidi text without interpreting HTML', () => {
    expect(toSafeMcpDisplayText('<img src=x>\u0000\u202eSAFE\u2066')).toBe('<img src=x>SAFE')
  })

  it('uses only the frozen CAS identity for a mutation precondition', () => {
    expect(
      toMcpPrecondition({
        schemaVersion: 1,
        serverId: SERVER_ID,
        displayName: 'fixture',
        scope: 'user',
        source: 'userManual',
        transport: 'stdio',
        trust: 'untrusted',
        approvalMode: 'prompt',
        registryRevision: 7,
        configEpoch: CONFIG_EPOCH,
        configDigest: DIGEST,
        state: 'disabled',
        enabled: false,
        launchAuthorizationState: 'required',
        catalogGeneration: 0,
        catalogCompleteness: 'failed',
        toolCount: 0,
        activeCallCount: 0,
        updatedAtMs: 1
      })
    ).toEqual({
      expectedRegistryRevision: 7,
      expectedConfigEpoch: CONFIG_EPOCH,
      expectedConfigDigest: DIGEST
    })
  })

  it('accepts empty and shell-looking argv as opaque values', () => {
    expect(
      validateMcpDraft(
        {
          approvalMode: 'prompt',
          arguments: ['', 'two words', '$(never-run); | still-one-argv'],
          cwd: '/tmp',
          displayName: 'fixture',
          executable: '/usr/bin/fixture'
        },
        translate
      )
    ).toEqual([])
  })

  it('rejects NUL without logging or returning the offending scalar', () => {
    const errors = validateMcpDraft(
      {
        approvalMode: 'prompt',
        arguments: ['fixed-canary\u0000must-not-cross'],
        cwd: '/tmp',
        displayName: 'fixture',
        executable: '/usr/bin/fixture'
      },
      translate
    )
    expect(errors).toContain('mcp.form.errorNul')
    expect(errors.join(' ')).not.toContain('fixed-canary')
  })
})

describe('MCP Renderer structured errors', () => {
  it('parses only the strict, safe Host error projection', () => {
    const details = getMcpManagementErrorDetails(
      new HostInvocationError({
        message: 'outer',
        data: {
          schemaVersion: 1,
          type: 'mcpManagement',
          operation: 'update',
          code: 'conflict',
          recovery: 'refresh',
          message: 'Configuration changed.',
          serverId: SERVER_ID,
          currentRegistryRevision: 9
        }
      })
    )
    expect(details).toEqual({
      code: 'conflict',
      currentRegistryRevision: 9,
      message: 'Configuration changed.',
      operation: 'update',
      recovery: 'refresh',
      serverId: SERVER_ID
    })
    expect(shouldRefreshMcpAfterError(details)).toBe(true)
  })

  it('fails closed when error data includes a secret-shaped extra field', () => {
    const details = getMcpManagementErrorDetails(
      new HostInvocationError({
        message: 'fixed-canary-must-not-cross',
        data: {
          schemaVersion: 1,
          type: 'mcpManagement',
          operation: 'add',
          code: 'invalidInput',
          recovery: 'fixInput',
          message: 'Safe message',
          rawArguments: 'fixed-canary-must-not-cross'
        }
      })
    )
    expect(details.message).toBe('')
    expect(details.message).not.toContain('fixed-canary')
  })

  it('recognizes only structured launch-authorization recovery errors', () => {
    const details = getMcpManagementErrorDetails(
      new HostInvocationError({
        message: 'outer',
        data: {
          schemaVersion: 1,
          type: 'mcpManagement',
          operation: 'enable',
          code: 'authorizationRequired',
          recovery: 'requestLaunchAuthorization',
          message: 'Launch authorization is required.',
          serverId: SERVER_ID,
          currentRegistryRevision: 9
        }
      })
    )

    expect(mcpOperationNeedsLaunchAuthorization(details)).toBe(true)
    expect(mcpOperationNeedsLaunchAuthorization({ message: '' })).toBe(false)
  })
})
