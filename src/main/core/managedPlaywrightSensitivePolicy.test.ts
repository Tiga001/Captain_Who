import { describe, expect, it } from 'vitest'
import type { ManagedPlaywrightAuthorizationContext } from '@mycopilot/protocol'

import type { ManagedPlaywrightToolManifestEntry } from '../mcp/managedPlaywrightManifest'
import {
  canonicalSha256,
  ManagedPlaywrightSensitiveGrantError,
  sensitiveBindingScopeForInvocation,
  sensitivePolicyForTool,
  sensitiveToolNeedsGrant,
  validateSensitiveToolGrant
} from '../mcp/managedPlaywrightSensitivePolicy'

const ORIGIN = 'https://mail.example.test'
const ARGS = {
  function: '() => document.title',
  call_reason: 'Read the current page title.'
}
const REVIEWED = {
  rawName: 'browser_evaluate',
  modelName: 'browser_evaluate',
  description: 'Evaluate page code',
  handlingMode: 'approval_required',
  exposed: true,
  safety: 'destructive',
  inputSchema: {},
  schemaDigest: `sha256:${'a'.repeat(64)}`,
  upstreamSchemaDigest: `sha256:${'b'.repeat(64)}`,
  reasonCode: 'sensitive_data_or_file_boundary',
  constraints: []
} satisfies ManagedPlaywrightToolManifestEntry

const COOKIE_SET_REVIEWED = {
  ...REVIEWED,
  rawName: 'browser_cookie_set',
  modelName: 'browser_cookie_set',
  description: 'Set a managed browser cookie'
} satisfies ManagedPlaywrightToolManifestEntry

function authorization(
  overrides: Partial<ManagedPlaywrightAuthorizationContext['builtinToolGrant']> = {}
): ManagedPlaywrightAuthorizationContext {
  const policy = sensitivePolicyForTool('browser_evaluate')!
  const argumentsDigest = canonicalSha256(ARGS)
  const targetBindingDigest = `sha256:${'d'.repeat(64)}`
  return {
    runId: 'run-1',
    capabilityId: 'browser_automation',
    activationId: '4af35bbd-cb1e-4b20-a021-92cd6b160829',
    manifestDigest: `sha256:${'c'.repeat(64)}`,
    policyRevision: 1,
    grantExpiresAtMs: 2_000_000,
    invocationId: 'c12f8536-faf9-439e-8e5a-a76049074e73',
    callId: 'call-1',
    triggerToolName: 'browser_evaluate',
    callReason: 'Read the current page title.',
    builtinToolGrant: {
      grantId: 'fe9c2750-fddf-4d3e-961f-55f6c0e17395',
      approvalId: 'ed14d2ba-9dc6-40e0-bfc7-22e71cad7fb8',
      argumentsDigest,
      targetBindingId: '398a919a-7b03-4234-b82c-a84498cf18ac',
      targetBindingDigest,
      resourceScopeDigest: canonicalSha256({
        schemaVersion: 2,
        toolId: 'browser_evaluate',
        argumentsDigest,
        origin: ORIGIN,
        riskKinds: [...policy.riskKinds],
        scope: policy.scope,
        targetBindingDigest
      }),
      origin: ORIGIN,
      riskKinds: ['page_script_execution'],
      expiresAtMs: 1_900_000,
      ...overrides
    }
  }
}

function scopedAuthorization(
  toolName: string,
  argumentsValue: Readonly<Record<string, unknown>>,
  origin: string | null,
  scope: 'managed_surface' | 'managed_browser_profile'
): ManagedPlaywrightAuthorizationContext {
  const policy = sensitivePolicyForTool(toolName)!
  const argumentsDigest = canonicalSha256(argumentsValue)
  const targetBindingDigest = `sha256:${'9'.repeat(64)}`
  return {
    ...authorization(),
    triggerToolName: toolName,
    builtinToolGrant: {
      ...authorization().builtinToolGrant!,
      argumentsDigest,
      targetBindingDigest,
      origin,
      riskKinds: [...policy.riskKinds],
      resourceScopeDigest: canonicalSha256({
        schemaVersion: 2,
        toolId: toolName,
        argumentsDigest,
        origin,
        riskKinds: [...policy.riskKinds],
        scope,
        targetBindingDigest
      })
    }
  }
}

describe('managed Playwright sensitive Tool policy', () => {
  it('classifies every approval-required manifest Tool and no automatic Tool', () => {
    const names = [
      'browser_cookie_clear',
      'browser_cookie_delete',
      'browser_cookie_get',
      'browser_cookie_list',
      'browser_cookie_set',
      'browser_drop',
      'browser_evaluate',
      'browser_file_upload',
      'browser_localstorage_clear',
      'browser_localstorage_delete',
      'browser_localstorage_get',
      'browser_localstorage_list',
      'browser_localstorage_set',
      'browser_network_request',
      'browser_sessionstorage_clear',
      'browser_sessionstorage_delete',
      'browser_sessionstorage_get',
      'browser_sessionstorage_list',
      'browser_sessionstorage_set',
      'browser_set_storage_state',
      'browser_storage_state'
    ]
    expect(names.every((name) => sensitivePolicyForTool(name) !== undefined)).toBe(true)
    expect(new Set(names).size).toBe(21)
    expect(sensitivePolicyForTool('browser_click')).toBeUndefined()
    expect(sensitivePolicyForTool('browser_run_code_unsafe')).toBeUndefined()
    expect(sensitivePolicyForTool('browser_set_storage_state')?.riskKinds).toEqual([
      'file_read',
      'cookie_write',
      'local_storage_write',
      'storage_state_import'
    ])
    expect(sensitivePolicyForTool('browser_storage_state')?.riskKinds).toEqual([
      'file_write',
      'cookie_read',
      'local_storage_read',
      'storage_state_export'
    ])
    for (const name of [
      'browser_cookie_clear',
      'browser_cookie_delete',
      'browser_cookie_get',
      'browser_cookie_list',
      'browser_cookie_set',
      'browser_set_storage_state',
      'browser_storage_state'
    ]) {
      expect(sensitivePolicyForTool(name)?.scope).toBe('managed_browser_profile')
    }
    for (const name of [
      'browser_drop',
      'browser_evaluate',
      'browser_file_upload',
      'browser_network_request'
    ]) {
      expect(sensitivePolicyForTool(name)?.scope).toBe('managed_surface')
    }
    expect(sensitiveBindingScopeForInvocation('browser_cookie_list', {})).toBe(
      'managed_browser_profile'
    )
    expect(
      sensitiveBindingScopeForInvocation('browser_cookie_set', { domain: 'example.test' })
    ).toBe('managed_browser_profile')
    expect(sensitiveBindingScopeForInvocation('browser_cookie_set', {})).toBe('managed_surface')
    expect(sensitiveBindingScopeForInvocation('browser_evaluate', ARGS)).toBe('managed_surface')
  })

  it('uses the same recursively canonical argument digest regardless of object key order', () => {
    expect(canonicalSha256({ a: 1, b: { x: 2 } })).toBe(canonicalSha256({ b: { x: 2 }, a: 1 }))
    expect(canonicalSha256({ a: 1 })).not.toBe(canonicalSha256({ a: 2 }))
    const argumentsDigest = canonicalSha256(ARGS)
    expect(argumentsDigest).toBe(
      'sha256:44679ad41b476dfb14d002b9d22a010f72f3c60ae4d060868c9ceacab2aabcd1'
    )
    expect(
      canonicalSha256({
        schemaVersion: 1,
        toolId: 'browser_evaluate',
        argumentsDigest,
        origin: ORIGIN,
        riskKinds: ['page_script_execution'],
        scope: 'managed_surface'
      })
    ).toBe('sha256:7a94511d5d3fa315c32237ca09c5bc7510fbe7cec14a40456b578f8e55181c7a')

    const cookieArguments = {
      call_reason: 'List reviewed managed browser cookies.'
    }
    const cookieArgumentsDigest = canonicalSha256(cookieArguments)
    expect(cookieArgumentsDigest).toBe(
      'sha256:7eedba5a990eb2faeeb27bebfccd4ac2b5fa1b6237b308ccaf402fc5772c72b7'
    )
    expect(
      canonicalSha256({
        schemaVersion: 1,
        toolId: 'browser_cookie_list',
        argumentsDigest: cookieArgumentsDigest,
        origin: null,
        riskKinds: ['cookie_read'],
        scope: 'managed_browser_profile'
      })
    ).toBe('sha256:e94817429ef8eba75b2b368825326e524b43f524c5dc38d9961f69e2d32f5bb2')
  })

  it('requires approval dynamically only for file-bearing drop and upload dispatches', () => {
    expect(
      sensitiveToolNeedsGrant('browser_drop', {
        target: 'editor',
        data: { 'text/plain': 'hello' }
      })
    ).toBe(false)
    expect(
      sensitiveToolNeedsGrant('browser_drop', {
        target: 'editor',
        paths: ['browser-file:4af35bbd-cb1e-4b20-a021-92cd6b160829']
      })
    ).toBe(true)
    expect(sensitiveToolNeedsGrant('browser_file_upload', {})).toBe(false)
    expect(
      sensitiveToolNeedsGrant('browser_file_upload', {
        paths: ['browser-file:4af35bbd-cb1e-4b20-a021-92cd6b160829']
      })
    ).toBe(true)
  })

  it('revalidates exact args, scope, risks, expiry and active origin before dispatch', () => {
    const consumed = new Set<string>()
    const lease = validateSensitiveToolGrant({
      reviewed: REVIEWED,
      modelArguments: ARGS,
      authorizationContext: authorization(),
      activeOrigin: ORIGIN,
      consumedGrantIds: consumed,
      now: 1_800_000
    })
    expect(lease?.grant.riskKinds).toEqual(['page_script_execution'])
    expect(consumed.size).toBe(0)
    lease?.markDispatched()
    expect(consumed.size).toBe(1)
    expect(() => lease?.markDispatched()).toThrow(ManagedPlaywrightSensitiveGrantError)

    expect(() =>
      validateSensitiveToolGrant({
        reviewed: REVIEWED,
        modelArguments: { ...ARGS, function: '() => document.cookie' },
        authorizationContext: authorization(),
        activeOrigin: ORIGIN,
        consumedGrantIds: new Set(),
        now: 1_800_000
      })
    ).toThrow(ManagedPlaywrightSensitiveGrantError)
    expect(() =>
      validateSensitiveToolGrant({
        reviewed: REVIEWED,
        modelArguments: ARGS,
        authorizationContext: authorization(),
        activeOrigin: 'https://other.example.test',
        consumedGrantIds: new Set(),
        now: 1_800_000
      })
    ).toThrow('mcp.builtin_playwright.sensitive_grant_origin_drifted')
    expect(() =>
      validateSensitiveToolGrant({
        reviewed: REVIEWED,
        modelArguments: ARGS,
        authorizationContext: authorization({ expiresAtMs: 1_700_000 }),
        activeOrigin: ORIGIN,
        consumedGrantIds: new Set(),
        now: 1_800_000
      })
    ).toThrow('mcp.builtin_playwright.sensitive_grant_expired')
  })

  it('binds cookie_set scope to whether fixed Playwright derives the domain from currentTab', () => {
    const pageArguments = {
      name: 'fixture',
      value: 'reviewed',
      call_reason: 'Set a cookie using the frozen current page domain.'
    }
    const profileArguments = {
      ...pageArguments,
      domain: 'fixture.invalid'
    }
    expect(
      validateSensitiveToolGrant({
        reviewed: COOKIE_SET_REVIEWED,
        modelArguments: pageArguments,
        authorizationContext: scopedAuthorization(
          'browser_cookie_set',
          pageArguments,
          ORIGIN,
          'managed_surface'
        ),
        activeOrigin: ORIGIN,
        consumedGrantIds: new Set(),
        now: 1_800_000
      })
    ).toBeDefined()
    expect(
      validateSensitiveToolGrant({
        reviewed: COOKIE_SET_REVIEWED,
        modelArguments: profileArguments,
        authorizationContext: scopedAuthorization(
          'browser_cookie_set',
          profileArguments,
          null,
          'managed_browser_profile'
        ),
        activeOrigin: null,
        consumedGrantIds: new Set(),
        now: 1_800_000
      })
    ).toBeDefined()

    for (const [argumentsValue, activeOrigin, wrongScope] of [
      [pageArguments, ORIGIN, 'managed_browser_profile'],
      [profileArguments, null, 'managed_surface']
    ] as const) {
      expect(() =>
        validateSensitiveToolGrant({
          reviewed: COOKIE_SET_REVIEWED,
          modelArguments: argumentsValue,
          authorizationContext: scopedAuthorization(
            'browser_cookie_set',
            argumentsValue,
            activeOrigin,
            wrongScope
          ),
          activeOrigin,
          consumedGrantIds: new Set(),
          now: 1_800_000
        })
      ).toThrow('mcp.builtin_playwright.sensitive_grant_drifted')
    }
  })

  it('fails closed when a required grant is missing or attached to an automatic MIME drop', () => {
    expect(() =>
      validateSensitiveToolGrant({
        reviewed: REVIEWED,
        modelArguments: ARGS,
        authorizationContext: { ...authorization(), builtinToolGrant: undefined },
        activeOrigin: ORIGIN,
        consumedGrantIds: new Set(),
        now: 1_800_000
      })
    ).toThrow('mcp.builtin_playwright.sensitive_grant_missing')

    const drop = { ...REVIEWED, rawName: 'browser_drop', modelName: 'browser_drop' }
    expect(() =>
      validateSensitiveToolGrant({
        reviewed: drop,
        modelArguments: {
          target: 'editor',
          data: { 'text/plain': 'hello' },
          call_reason: 'Drop text.'
        },
        authorizationContext: authorization(),
        activeOrigin: '',
        consumedGrantIds: new Set(),
        now: 1_800_000
      })
    ).toThrow('mcp.builtin_playwright.sensitive_grant_drifted')
  })
})
