import { beforeEach, describe, expect, it, vi } from 'vitest'

const network = vi.hoisted(() => ({
  resolveProxy: vi.fn(),
  setProxy: vi.fn(async () => {}),
  fromPartition: vi.fn()
}))
vi.mock('electron', () => ({ session: { fromPartition: network.fromPartition } }))

import {
  parseNetworkRouteRequest,
  parseSystemProxyRoute,
  resolveSystemNetworkRoute
} from '../network/NetworkRoutes'

describe('Host system network routing', () => {
  beforeEach(() => {
    network.fromPartition.mockReturnValue(network)
    network.resolveProxy.mockReset()
  })

  it.each([
    ['DIRECT', { kind: 'direct' }],
    ['PROXY 127.0.0.1:7897; DIRECT', { kind: 'httpProxy', host: '127.0.0.1', port: 7897 }],
    ['HTTPS proxy.example:443', { kind: 'httpsProxy', host: 'proxy.example', port: 443 }],
    ['SOCKS5 [::1]:7897', { kind: 'socks5Proxy', host: '::1', port: 7897 }],
    ['SOCKS localhost:1080', { kind: 'socks4Proxy', host: 'localhost', port: 1080 }],
    ['SOCKS4 localhost:1080', { kind: 'socks4Proxy', host: 'localhost', port: 1080 }],
    ['QUIC proxy.example:443', null],
    ['', null],
    ['PROXY user:secret@proxy.example:8080', null],
    ['PROXY proxy.example:0', null],
    ['PROXY proxy.example:65536', null]
  ])('parses system route %s without inventing a direct fallback', (value, expected) => {
    expect(parseSystemProxyRoute(value)).toEqual(expected)
  })

  it('resolves every URL again so proxy changes and PAC bypasses apply without restart', async () => {
    network.resolveProxy
      .mockResolvedValueOnce('PROXY 127.0.0.1:7897')
      .mockResolvedValueOnce('SOCKS5 127.0.0.1:7898')
      .mockResolvedValueOnce('DIRECT')
    expect(await resolveSystemNetworkRoute('https://cdn.example/a')).toEqual({
      kind: 'httpProxy',
      host: '127.0.0.1',
      port: 7897
    })
    expect(await resolveSystemNetworkRoute('https://cdn.example/a')).toEqual({
      kind: 'socks5Proxy',
      host: '127.0.0.1',
      port: 7898
    })
    expect(await resolveSystemNetworkRoute('https://local.example/b')).toEqual({ kind: 'direct' })
    expect(network.resolveProxy.mock.calls).toEqual([
      ['https://cdn.example/a'],
      ['https://cdn.example/a'],
      ['https://local.example/b']
    ])
    expect(network.setProxy).toHaveBeenCalledWith({ mode: 'system' })
    expect(network.fromPartition).toHaveBeenCalledWith('captain-who-network-routes', {
      cache: false
    })
  })

  it('propagates route resolution failure instead of bypassing the proxy', async () => {
    network.resolveProxy.mockRejectedValueOnce(new Error('PAC unavailable'))
    await expect(resolveSystemNetworkRoute('https://example.test')).rejects.toThrow(
      'PAC unavailable'
    )
  })

  it('accepts only bounded HTTP route requests bound to a Core-generated request id', () => {
    const requestId = '5f5d9ec8-4040-49ba-a776-b5fe24e4a199'
    expect(
      parseNetworkRouteRequest({ requestId, url: 'https://example.test/image?token=private' })
    ).toEqual({ requestId, url: 'https://example.test/image?token=private' })
    for (const value of [
      null,
      {},
      { requestId, url: 'file:///etc/hosts' },
      { requestId, url: 'https://example.test/' + 'a'.repeat(32768) },
      { requestId: 'wrong', url: 'https://example.test' }
    ]) {
      expect(parseNetworkRouteRequest(value)).toBeNull()
    }
  })
})
