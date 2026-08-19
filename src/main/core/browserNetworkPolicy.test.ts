import { describe, expect, it, vi } from 'vitest'

import {
  BrowserNetworkPolicy,
  ElectronSessionDnsResolver,
  classifyAddress,
  type BrowserDnsResolver
} from '../browser/BrowserNetworkPolicy'

class SequenceResolver implements BrowserDnsResolver {
  private index = 0

  constructor(private readonly answers: readonly (readonly string[])[]) {}

  async resolve(): Promise<readonly string[]> {
    const answer = this.answers[Math.min(this.index, this.answers.length - 1)] ?? []
    this.index += 1
    return answer
  }
}

describe('BrowserNetworkPolicy', () => {
  it('allows ordinary public HTTPS without an approval', async () => {
    const policy = new BrowserNetworkPolicy({
      dnsResolver: new SequenceResolver([['93.184.216.34']])
    })

    await expect(
      policy.assess('https://example.test/path?token=secret#fragment')
    ).resolves.toMatchObject({
      disposition: 'allow',
      destination: {
        displayUrl: 'https://example.test/path',
        origin: 'https://example.test',
        scheme: 'https',
        host: 'example.test',
        port: 443,
        addressClass: 'public'
      },
      riskKinds: []
    })
  })

  it.each([
    ['http://public.test/', ['insecure_http']],
    ['https://public.test:8443/', ['non_standard_port']],
    ['https://user:password@public.test/', ['url_userinfo']],
    ['http://localhost/', ['insecure_http', 'localhost', 'loopback', 'local_service_request']],
    ['http://127.0.0.1/', ['insecure_http', 'loopback', 'local_service_request']],
    ['http://[::1]/', ['insecure_http', 'loopback', 'local_service_request']],
    ['https://10.0.0.1/', ['private_network', 'local_service_request']],
    ['https://192.168.1.5/', ['private_network', 'local_service_request']],
    ['https://169.254.1.1/', ['link_local', 'local_service_request']],
    ['https://169.254.169.254/', ['cloud_metadata', 'local_service_request']],
    ['https://[fd00:ec2::254]/', ['cloud_metadata', 'local_service_request']],
    ['https://[fe80::1]/', ['link_local', 'local_service_request']],
    ['https://[fd12::1]/', ['private_network', 'local_service_request']]
  ])('classifies risky destination %s', async (url, expectedRisks) => {
    const policy = new BrowserNetworkPolicy({
      dnsResolver: new SequenceResolver([['93.184.216.34']])
    })

    const result = await policy.assess(url)

    expect(result.disposition).toBe('approval_required')
    if (result.disposition === 'approval_required') {
      expect(result.riskKinds).toEqual(expectedRisks)
    }
  })

  it('classifies a public hostname resolving to a private address', async () => {
    const policy = new BrowserNetworkPolicy({
      dnsResolver: new SequenceResolver([['10.2.3.4']])
    })

    const result = await policy.assess('https://rebinding.test/')

    expect(result).toMatchObject({
      disposition: 'approval_required',
      destination: { addressClass: 'private' }
    })
    if (result.disposition === 'approval_required') {
      expect(result.riskKinds).toEqual([
        'private_network',
        'dns_private_resolution',
        'local_service_request'
      ])
    }
  })

  it('produces a new identity when DNS rebinding changes the answer', async () => {
    const policy = new BrowserNetworkPolicy({
      dnsResolver: new SequenceResolver([['93.184.216.34'], ['127.0.0.1']])
    })

    const publicResult = await policy.assess('https://rebinding.test/')
    const reboundResult = await policy.assess('https://rebinding.test/')

    expect(publicResult.disposition).toBe('allow')
    expect(reboundResult.disposition).toBe('approval_required')
    if (publicResult.disposition !== 'deny' && reboundResult.disposition === 'approval_required') {
      expect(reboundResult.destination.resolutionFingerprint).not.toBe(
        publicResult.destination.resolutionFingerprint
      )

      expect(reboundResult.destination.addressClass).toBe('loopback')
    }
  })

  it('treats localhost Renderer origins as equivalent to resolved loopback addresses', async () => {
    const policy = new BrowserNetworkPolicy({
      blockedOrigins: ['http://localhost:5173/'],
      dnsResolver: new SequenceResolver([['127.0.0.1']])
    })

    await expect(policy.assess('http://renderer-alias.test:5173/private')).resolves.toEqual({
      disposition: 'deny',
      code: 'main_renderer'
    })
  })

  it('never includes URL credentials, query, or fragment in the display projection', async () => {
    const policy = new BrowserNetworkPolicy({
      dnsResolver: new SequenceResolver([['93.184.216.34']])
    })

    const first = await policy.assess('https://alice:secret@public.test/path?token=canary#private')
    const second = await policy.assess('https://alice:other@public.test/path?token=different')

    expect(first.disposition).toBe('approval_required')
    expect(second.disposition).toBe('approval_required')
    if (first.disposition === 'approval_required' && second.disposition === 'approval_required') {
      expect(first.destination.displayUrl).toBe('https://public.test/path')
      expect(JSON.stringify(first)).not.toContain('secret')
      expect(JSON.stringify(first)).not.toContain('canary')
      expect(first.destination.targetDigest).not.toBe(second.destination.targetDigest)
    }
  })

  it.each([
    ['file:///private/file', 'privileged_electron'],
    ['chrome://settings/', 'privileged_electron'],
    ['devtools://devtools/bundled/', 'privileged_electron'],
    ['javascript:alert(1)', 'privileged_electron'],
    ['data:text/html,hello', 'unsupported_scheme']
  ])('denies non-reviewable scheme %s before DNS', async (url, code) => {
    const resolver = new SequenceResolver([['93.184.216.34']])
    const policy = new BrowserNetworkPolicy({ dnsResolver: resolver })

    await expect(policy.assess(url)).resolves.toEqual({ disposition: 'deny', code })
  })

  it('denies the exact Renderer, debug, and MCP control origins as Host boundaries', async () => {
    const policy = new BrowserNetworkPolicy({
      blockedOrigins: ['http://127.0.0.1:5173/index.html'],
      debugEndpoints: [{ host: '127.0.0.1', port: 9222 }],
      mcpControlEndpoints: [{ host: '127.0.0.1', port: 8765 }],
      dnsResolver: new SequenceResolver([['127.0.0.1']])
    })

    await expect(policy.assess('http://127.0.0.1:5173/private')).resolves.toEqual({
      disposition: 'deny',
      code: 'main_renderer'
    })
    await expect(policy.assess('http://127.0.0.1:9222/json')).resolves.toEqual({
      disposition: 'deny',
      code: 'internal_debug'
    })
    await expect(policy.assess('http://127.0.0.1:8765/rpc')).resolves.toEqual({
      disposition: 'deny',
      code: 'mcp_control'
    })
  })

  it.each([
    [5173, 'main_renderer'],
    [9222, 'internal_debug'],
    [8765, 'mcp_control']
  ])('denies a public-looking name resolving to a Host endpoint on port %i', async (port, code) => {
    const policy = new BrowserNetworkPolicy({
      blockedOrigins: ['http://127.0.0.1:5173/index.html'],
      debugEndpoints: [{ host: '127.0.0.1', port: 9222 }],
      mcpControlEndpoints: [{ host: '127.0.0.1', port: 8765 }],
      dnsResolver: new SequenceResolver([['127.0.0.1']])
    })

    await expect(policy.assess(`http://rebinding.test:${port}/private`)).resolves.toEqual({
      disposition: 'deny',
      code
    })
  })

  it('fails closed when DNS cannot prove the destination boundary', async () => {
    const resolver: BrowserDnsResolver = {
      async resolve(): Promise<readonly string[]> {
        throw new Error('fixture resolution failed')
      }
    }
    const policy = new BrowserNetworkPolicy({ dnsResolver: resolver })

    await expect(policy.assess('https://unresolved.test/')).resolves.toEqual({
      disposition: 'deny',
      code: 'resolution_unavailable'
    })
  })

  it('hard-bounds a resolver that ignores AbortSignal', async () => {
    vi.useFakeTimers()
    try {
      const resolver: BrowserDnsResolver = {
        async resolve(): Promise<readonly string[]> {
          return await new Promise<readonly string[]>(() => undefined)
        }
      }
      const policy = new BrowserNetworkPolicy({ dnsResolver: resolver, dnsTimeoutMs: 25 })
      const assessment = policy.assess('https://hung-resolver.test/')

      await vi.advanceTimersByTimeAsync(25)

      await expect(assessment).resolves.toEqual({
        disposition: 'deny',
        code: 'resolution_unavailable'
      })
    } finally {
      vi.useRealTimers()
    }
  })

  it('single-flights concurrent lookups for one host and deletes the settled slot', async () => {
    let release!: (addresses: readonly string[]) => void
    const resolve = vi.fn(
      async () =>
        await new Promise<readonly string[]>((done) => {
          release = done
        })
    )
    const policy = new BrowserNetworkPolicy({ dnsResolver: { resolve } })
    const first = policy.assess('https://shared-host.test/one')
    const second = policy.assess('https://shared-host.test/two')
    await vi.waitFor(() => expect(resolve).toHaveBeenCalledOnce())

    release(['93.184.216.34'])

    await expect(Promise.all([first, second])).resolves.toMatchObject([
      { disposition: 'allow' },
      { disposition: 'allow' }
    ])
    expect(policy.snapshot().pendingResolutions).toBe(0)
  })

  it('fails closed when unique DNS work reaches bounded admission', async () => {
    const releases: Array<(addresses: readonly string[]) => void> = []
    const resolver: BrowserDnsResolver = {
      async resolve(): Promise<readonly string[]> {
        return await new Promise<readonly string[]>((resolve) => releases.push(resolve))
      }
    }
    const policy = new BrowserNetworkPolicy({ dnsResolver: resolver })
    const admitted = Array.from({ length: 32 }, (_, index) =>
      policy.assess(`https://dns-storm-${index}.test/`)
    )
    await vi.waitFor(() => expect(policy.snapshot().pendingResolutions).toBe(32))

    await expect(policy.assess('https://dns-overflow.test/')).resolves.toEqual({
      disposition: 'deny',
      code: 'resolution_unavailable'
    })

    for (const release of releases) release(['93.184.216.34'])
    await Promise.all(admitted)
    expect(policy.snapshot().pendingResolutions).toBe(0)
  })

  it('uses the managed Electron session resolver with cache bypass options', async () => {
    const resolveHost = vi.fn(async () => ({
      endpoints: [{ address: '93.184.216.34', family: 'ipv4' as const, port: 0 }]
    }))
    const resolver = new ElectronSessionDnsResolver({ resolveHost })

    await expect(resolver.resolve('fixture.test', new AbortController().signal)).resolves.toEqual([
      '93.184.216.34'
    ])
    expect(resolveHost).toHaveBeenCalledWith('fixture.test', {
      cacheUsage: 'disallowed',
      source: 'any',
      secureDnsPolicy: 'allow'
    })
  })

  it.each([
    ['8.8.8.8', 'public'],
    ['127.0.0.2', 'loopback'],
    ['10.0.0.1', 'private'],
    ['169.254.8.8', 'link_local'],
    ['169.254.169.254', 'cloud_metadata'],
    ['::1', 'loopback'],
    ['fe80::1', 'link_local'],
    ['fd00::1', 'private'],
    ['::ffff:127.0.0.1', 'loopback'],
    ['::7f00:1', 'loopback'],
    ['::0a00:1', 'private']
  ])('classifies address %s as %s', (address, expected) => {
    expect(classifyAddress(address)).toBe(expected)
  })

  it('does not truncate an oversized DNS answer set before boundary classification', async () => {
    const answers = Array.from({ length: 16 }, (_, index) => `8.8.8.${index + 1}`)
    answers.push('127.0.0.1')
    const policy = new BrowserNetworkPolicy({
      debugEndpoints: [{ host: '127.0.0.1', port: 9222 }],
      dnsResolver: new SequenceResolver([answers])
    })

    await expect(policy.assess('https://many-addresses.test:9222/')).resolves.toEqual({
      disposition: 'deny',
      code: 'resolution_overflow'
    })
  })
})
