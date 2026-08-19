import { describe, expect, it } from 'vitest'
import type { ManagedPlaywrightAuthorizationContext } from '@mycopilot/protocol'

import type { ManagedPlaywrightToolManifestEntry } from '../mcp/managedPlaywrightManifest'
import {
  canonicalSha256,
  ManagedPlaywrightSensitiveGrantError,
  sensitivePolicyForTool,
  sensitiveToolNeedsGrant,
  validateSensitiveToolGrant
} from '../mcp/managedPlaywrightSensitivePolicy'

const ORIGIN = 'https://mail.example.test'
const ARGS = {
  function: '() => document.title',
  approval_origin: ORIGIN,
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
  })

  it('uses the same recursively canonical argument digest regardless of object key order', () => {
    expect(canonicalSha256({ a: 1, b: { x: 2 } })).toBe(canonicalSha256({ b: { x: 2 }, a: 1 }))
    expect(canonicalSha256({ a: 1 })).not.toBe(canonicalSha256({ a: 2 }))
    const argumentsDigest = canonicalSha256(ARGS)
    expect(argumentsDigest).toBe(
      'sha256:fe743397c82b02191460880e6714fb89675cf1e8148dbac7e4aa14e1ea969694'
    )
    expect(
      canonicalSha256({
        schemaVersion: 1,
        toolId: 'browser_evaluate',
        argumentsDigest,
        origin: ORIGIN,
        riskKinds: ['page_script_execution'],
        scope: 'page_script_execution'
      })
    ).toBe('sha256:c2e7c64206076f6d422b1526c49b2870c601106dc88bcea59102dc39d36556d1')

    const cookieArguments = {
      approval_origin: ORIGIN,
      call_reason: 'List reviewed managed browser cookies.'
    }
    const cookieArgumentsDigest = canonicalSha256(cookieArguments)
    expect(cookieArgumentsDigest).toBe(
      'sha256:85a6b62bc42b5e8f095052224a1d4f40b4366029e5e212c8b07b8679c22fa281'
    )
    expect(
      canonicalSha256({
        schemaVersion: 1,
        toolId: 'browser_cookie_list',
        argumentsDigest: cookieArgumentsDigest,
        origin: ORIGIN,
        riskKinds: ['cookie_read'],
        scope: 'managed_browser_profile'
      })
    ).toBe('sha256:867eef1812ae00959599b4cbc376c1608b217e1072552286b9f581e48f0f7ed7')
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
          approval_origin: ORIGIN,
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
