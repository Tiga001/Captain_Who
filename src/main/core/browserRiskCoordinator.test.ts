import { describe, expect, it, vi } from 'vitest'

import { BrowserNetworkPolicy, type BrowserDnsResolver } from '../browser/BrowserNetworkPolicy'
import {
  BrowserRiskCoordinator,
  type BrowserRiskAuthorizationContext,
  type BrowserRiskAuthorizationDecision,
  type BrowserRiskAuthorizationRequest,
  type BrowserRiskAuthorizer
} from '../browser/BrowserRiskCoordinator'

const CONTEXT: BrowserRiskAuthorizationContext = {
  runId: 'run-1',
  capabilityId: 'browser_automation',
  activationId: '123e4567-e89b-42d3-a456-426614174000',
  manifestDigest: `sha256:${'a'.repeat(64)}`,
  policyRevision: 7,
  grantExpiresAtMs: 2_000_000,
  invocationId: '123e4567-e89b-42d3-a456-426614174001',
  callId: 'call-1',
  triggerToolName: 'browser_navigate',
  callReason: 'Open the test page.'
}

class SequenceResolver implements BrowserDnsResolver {
  private index = 0

  constructor(private readonly answers: readonly (readonly string[])[]) {}

  async resolve(): Promise<readonly string[]> {
    const result = this.answers[Math.min(this.index, this.answers.length - 1)] ?? []
    this.index += 1
    return result
  }
}

class Authorizer implements BrowserRiskAuthorizer {
  readonly requests: BrowserRiskAuthorizationRequest[] = []

  constructor(
    private readonly decide: (
      request: BrowserRiskAuthorizationRequest
    ) => Promise<BrowserRiskAuthorizationDecision> | BrowserRiskAuthorizationDecision
  ) {}

  async authorize(
    request: BrowserRiskAuthorizationRequest
  ): Promise<BrowserRiskAuthorizationDecision> {
    this.requests.push(request)
    return await this.decide(request)
  }
}

function operation(coordinator: BrowserRiskCoordinator, signal?: AbortSignal) {
  return coordinator.beginOperation({
    authorizationContext: CONTEXT,
    parentRequestId: '123e4567-e89b-42d3-a456-426614174002',
    signal
  })
}

describe('BrowserRiskCoordinator', () => {
  it('does not contact Core for ordinary public HTTPS', async () => {
    const authorizer = new Authorizer(() => ({ decision: 'approved', grantId: 'unused' }))
    const coordinator = new BrowserRiskCoordinator({
      authorizer,
      now: () => 1_000_000,
      policy: new BrowserNetworkPolicy({
        dnsResolver: new SequenceResolver([['93.184.216.34']])
      })
    })

    await operation(coordinator).check({
      url: 'https://public.test/',
      trigger: 'tool_argument',
      dispatchCertainty: 'definitely_not_dispatched'
    })

    expect(authorizer.requests).toHaveLength(0)
  })

  it('requests approval and re-resolves before releasing a risky request', async () => {
    const authorizer = new Authorizer(() => ({
      decision: 'approved',
      grantId: '123e4567-e89b-42d3-a456-426614174003'
    }))
    const coordinator = new BrowserRiskCoordinator({
      authorizer,
      now: () => 1_000_000,
      policy: new BrowserNetworkPolicy({
        dnsResolver: new SequenceResolver([['10.0.0.4'], ['10.0.0.4']])
      })
    })

    await operation(coordinator).check({
      url: 'https://private.test/path?secret=canary',
      trigger: 'main_frame',
      dispatchCertainty: 'definitely_not_dispatched'
    })

    expect(authorizer.requests).toHaveLength(1)
    expect(authorizer.requests[0]).toMatchObject({
      authorizationContext: CONTEXT,
      destination: {
        displayUrl: 'https://private.test/path',
        origin: 'https://private.test',
        addressClass: 'private'
      },
      riskKinds: ['private_network', 'dns_private_resolution', 'local_service_request'],
      trigger: 'main_frame',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    expect(JSON.stringify(authorizer.requests[0])).not.toContain('canary')
  })

  it('requires a fresh approval after DNS risk drift', async () => {
    const authorizer = new Authorizer(() => ({
      decision: 'approved',
      grantId: '123e4567-e89b-42d3-a456-426614174003'
    }))
    const coordinator = new BrowserRiskCoordinator({
      authorizer,
      now: () => 1_000_000,
      policy: new BrowserNetworkPolicy({
        dnsResolver: new SequenceResolver([
          ['93.184.216.34'],
          ['10.0.0.4'],
          ['10.0.0.4'],
          ['10.0.0.4']
        ])
      })
    })

    await operation(coordinator).check({
      url: 'http://rebinding.test/',
      trigger: 'redirect',
      dispatchCertainty: 'definitely_not_dispatched'
    })

    expect(authorizer.requests).toHaveLength(2)
    expect(authorizer.requests[0]?.destination.addressClass).toBe('public')
    expect(authorizer.requests[1]?.destination.addressClass).toBe('private')
  })

  it('does not release a public HTTPS fast path that rebinds to a private address', async () => {
    const authorizer = new Authorizer(() => ({
      decision: 'approved',
      grantId: '123e4567-e89b-42d3-a456-426614174003'
    }))
    const coordinator = new BrowserRiskCoordinator({
      authorizer,
      now: () => 1_000_000,
      policy: new BrowserNetworkPolicy({
        dnsResolver: new SequenceResolver([['93.184.216.34'], ['10.0.0.4'], ['10.0.0.4']])
      })
    })

    await operation(coordinator).check({
      url: 'https://rebinding.test/',
      trigger: 'main_frame',
      dispatchCertainty: 'definitely_not_dispatched'
    })

    expect(authorizer.requests).toHaveLength(1)
    expect(authorizer.requests[0]).toMatchObject({
      destination: { addressClass: 'private' },
      riskKinds: ['private_network', 'dns_private_resolution', 'local_service_request']
    })
  })

  it('returns a typed normal refusal input and records it for the MCP host', async () => {
    const authorizer = new Authorizer(() => ({
      decision: 'rejected',
      reason: 'Do not access the local service.\n'
    }))
    const coordinator = new BrowserRiskCoordinator({
      authorizer,
      now: () => 1_000_000,
      policy: new BrowserNetworkPolicy({
        dnsResolver: new SequenceResolver([['127.0.0.1']])
      })
    })
    const lease = operation(coordinator)

    await expect(
      lease.check({
        url: 'http://127.0.0.1:3000/',
        trigger: 'tool_argument',
        dispatchCertainty: 'definitely_not_dispatched'
      })
    ).rejects.toMatchObject({
      failure: {
        code: 'browser.risk_rejected',
        dispatchCertainty: 'definitely_not_dispatched',
        reason: 'Do not access the local service.'
      }
    })
    expect(lease.failure()).toEqual({
      code: 'browser.risk_rejected',
      dispatchCertainty: 'definitely_not_dispatched',
      reason: 'Do not access the local service.'
    })
  })

  it('deduplicates concurrent approval requests for the same origin and grant', async () => {
    let release!: (decision: BrowserRiskAuthorizationDecision) => void
    const pending = new Promise<BrowserRiskAuthorizationDecision>((resolve) => {
      release = resolve
    })
    const authorizer = new Authorizer(() => pending)
    const coordinator = new BrowserRiskCoordinator({
      authorizer,
      now: () => 1_000_000,
      policy: new BrowserNetworkPolicy({
        dnsResolver: new SequenceResolver([['10.0.0.4'], ['10.0.0.4'], ['10.0.0.4'], ['10.0.0.4']])
      })
    })
    const first = operation(coordinator).check({
      url: 'https://private.test/one',
      trigger: 'subresource',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    const second = operation(coordinator).check({
      url: 'https://private.test/two',
      trigger: 'subresource',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    await vi.waitFor(() => expect(authorizer.requests).toHaveLength(1))
    release({ decision: 'approved', grantId: '123e4567-e89b-42d3-a456-426614174003' })

    await expect(Promise.all([first, second])).resolves.toEqual([undefined, undefined])
    expect(coordinator.snapshot()).toEqual({ active: 0, inFlight: 0 })
  })

  it('fails closed on a non-reviewable Host boundary without contacting Core', async () => {
    const authorizer = new Authorizer(() => ({ decision: 'approved', grantId: 'unused' }))
    const coordinator = new BrowserRiskCoordinator({
      authorizer,
      now: () => 1_000_000,
      policy: new BrowserNetworkPolicy({
        blockedOrigins: ['http://127.0.0.1:5173'],
        dnsResolver: new SequenceResolver([['127.0.0.1']])
      })
    })

    await expect(
      operation(coordinator).check({
        url: 'http://renderer-alias.test:5173/private',
        trigger: 'new_window',
        dispatchCertainty: 'definitely_not_dispatched'
      })
    ).rejects.toMatchObject({
      failure: {
        code: 'browser.unsupported_host_boundary',
        dispatchCertainty: 'definitely_not_dispatched'
      }
    })
    expect(authorizer.requests).toHaveLength(0)
  })

  it('does not contact Core when DNS resolution is unavailable', async () => {
    const authorizer = new Authorizer(() => ({ decision: 'approved', grantId: 'unused' }))
    const coordinator = new BrowserRiskCoordinator({
      authorizer,
      now: () => 1_000_000,
      policy: new BrowserNetworkPolicy({
        dnsResolver: {
          async resolve(): Promise<readonly string[]> {
            throw new Error('fixture resolver unavailable')
          }
        }
      })
    })

    await expect(
      operation(coordinator).check({
        url: 'https://unresolved.test/',
        trigger: 'main_frame',
        dispatchCertainty: 'definitely_not_dispatched'
      })
    ).rejects.toMatchObject({
      failure: { code: 'browser.unsupported_host_boundary' }
    })
    expect(authorizer.requests).toHaveLength(0)
  })

  it('keeps an exact-action fingerprint stable for the same target and trigger', async () => {
    const authorizer = new Authorizer(() => ({
      decision: 'approved',
      grantId: '123e4567-e89b-42d3-a456-426614174003'
    }))
    const coordinator = new BrowserRiskCoordinator({
      authorizer,
      now: () => 1_000_000,
      policy: new BrowserNetworkPolicy({
        dnsResolver: new SequenceResolver([
          ['127.0.0.1'],
          ['127.0.0.1'],
          ['127.0.0.1'],
          ['127.0.0.1']
        ])
      })
    })
    const input = {
      url: 'http://127.0.0.1:3000/download?opaque=canary',
      trigger: 'download' as const,
      additionalRisks: ['file_download'] as const,
      dispatchCertainty: 'possibly_dispatched' as const
    }

    await operation(coordinator).check(input)
    await operation(coordinator).check(input)

    expect(authorizer.requests).toHaveLength(2)
    expect(authorizer.requests[0]?.destination.targetDigest).toBe(
      authorizer.requests[1]?.destination.targetDigest
    )
    expect(JSON.stringify(authorizer.requests)).not.toContain('canary')
  })

  it('cancels a pending approval and leaves no live waiter', async () => {
    const authorizer = new Authorizer(
      () => new Promise<BrowserRiskAuthorizationDecision>(() => undefined)
    )
    const coordinator = new BrowserRiskCoordinator({
      authorizer,
      now: () => 1_000_000,
      policy: new BrowserNetworkPolicy({
        dnsResolver: new SequenceResolver([['127.0.0.1']])
      })
    })
    const controller = new AbortController()
    const check = operation(coordinator, controller.signal).check({
      url: 'http://127.0.0.1:3000/',
      trigger: 'main_frame',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    await vi.waitFor(() => expect(coordinator.snapshot().active).toBe(1))

    controller.abort('task_cancelled')

    await expect(check).rejects.toMatchObject({
      failure: { code: 'browser.risk_cancelled' }
    })
    expect(coordinator.snapshot()).toEqual({ active: 0, inFlight: 0 })
  })

  it('does not expire a pending human approval when its runtime binding TTL elapses', async () => {
    vi.useFakeTimers()
    try {
      const authorizer = new Authorizer(
        () => new Promise<BrowserRiskAuthorizationDecision>(() => undefined)
      )
      const coordinator = new BrowserRiskCoordinator({
        authorizer,
        now: () => 1_000_000,
        policy: new BrowserNetworkPolicy({
          dnsResolver: new SequenceResolver([['127.0.0.1']])
        }),
        timeoutMs: 1_000
      })
      const controller = new AbortController()
      const check = operation(coordinator, controller.signal).check({
        url: 'http://127.0.0.1:3000/',
        trigger: 'main_frame',
        dispatchCertainty: 'definitely_not_dispatched'
      })
      await vi.waitFor(() => expect(coordinator.snapshot().active).toBe(1))

      await vi.advanceTimersByTimeAsync(2_000)

      expect(coordinator.snapshot()).toEqual({ active: 1, inFlight: 1 })
      controller.abort('task_cancelled')
      await expect(check).rejects.toMatchObject({
        failure: { code: 'browser.risk_cancelled' }
      })
    } finally {
      vi.useRealTimers()
    }
  })

  it('maps cancellation during DNS resolution to a typed pre-dispatch cancellation', async () => {
    let release!: (addresses: readonly string[]) => void
    const policy = new BrowserNetworkPolicy({
      dnsResolver: {
        async resolve(): Promise<readonly string[]> {
          return await new Promise<readonly string[]>((resolve) => {
            release = resolve
          })
        }
      }
    })
    const authorizer = new Authorizer(() => ({ decision: 'approved', grantId: 'unused' }))
    const coordinator = new BrowserRiskCoordinator({ authorizer, now: () => 1_000_000, policy })
    const controller = new AbortController()
    const check = operation(coordinator, controller.signal).check({
      url: 'https://pending-dns.test/',
      trigger: 'tool_argument',
      dispatchCertainty: 'definitely_not_dispatched'
    })

    controller.abort('task_cancelled')

    await expect(check).rejects.toMatchObject({
      failure: {
        code: 'browser.risk_cancelled',
        dispatchCertainty: 'definitely_not_dispatched'
      }
    })
    expect(authorizer.requests).toHaveLength(0)
    release(['93.184.216.34'])
    await vi.waitFor(() => expect(policy.snapshot().pendingResolutions).toBe(0))
  })

  it('settles every pending approval on shutdown', async () => {
    const authorizer = new Authorizer(
      () => new Promise<BrowserRiskAuthorizationDecision>(() => undefined)
    )
    const coordinator = new BrowserRiskCoordinator({
      authorizer,
      now: () => 1_000_000,
      policy: new BrowserNetworkPolicy({
        dnsResolver: new SequenceResolver([['127.0.0.1']])
      })
    })
    const check = operation(coordinator).check({
      url: 'http://127.0.0.1:3000/',
      trigger: 'main_frame',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    await vi.waitFor(() => expect(coordinator.snapshot().active).toBe(1))

    await coordinator.shutdown()

    await expect(check).rejects.toMatchObject({ failure: { code: 'browser.risk_cancelled' } })
    expect(coordinator.snapshot()).toEqual({ active: 0, inFlight: 0 })
  })
})
