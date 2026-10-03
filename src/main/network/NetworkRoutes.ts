import { isIP } from 'node:net'
import { session, type Session } from 'electron'

// Private stdio contract; never exposed to the renderer, model, or conversation trace.
export const NETWORK_ROUTE_RESOLVE_METHOD = 'host.networkRoute.resolve'
export const NETWORK_ROUTE_COMPLETE_METHOD = 'host.networkRoute.complete'
export type NetworkRoute =
  | { kind: 'direct' }
  | { kind: 'httpProxy' | 'httpsProxy' | 'socks4Proxy' | 'socks5Proxy'; host: string; port: number }

export interface NetworkRouteRequest {
  requestId: string
  url: string
}

export function parseNetworkRouteRequest(value: unknown): NetworkRouteRequest | null {
  if (!value || typeof value !== 'object') return null
  const request = value as Record<string, unknown>
  if (
    typeof request.requestId !== 'string' ||
    !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu.test(
      request.requestId
    ) ||
    typeof request.url !== 'string' ||
    request.url.length > 32 * 1024
  )
    return null
  try {
    const url = new URL(request.url)
    if (!['https:', 'http:'].includes(url.protocol) || !url.hostname) return null
  } catch {
    return null
  }
  return { requestId: request.requestId, url: request.url }
}

export function parseSystemProxyRoute(value: string): NetworkRoute | null {
  for (const entry of value.split(';').map((part) => part.trim())) {
    if (entry === 'DIRECT') return { kind: 'direct' }
    const match = /^(PROXY|HTTPS|SOCKS|SOCKS4|SOCKS5)\s+(\[[\da-f:]+\]|[^\s:/@?#]+):(\d+)$/iu.exec(
      entry
    )
    if (!match) continue
    const [, protocol, address, portValue] = match
    const host = address.startsWith('[') ? address.slice(1, -1) : address
    const port = Number(portValue)
    if (port < 1 || port > 65535 || host.length > 253) continue
    if (
      !isIP(host) &&
      !host
        .split('.')
        .every(
          (label) =>
            label.length > 0 && label.length <= 63 && /^[a-z\d](?:[a-z\d-]*[a-z\d])?$/iu.test(label)
        )
    )
      continue
    const kind = (
      {
        PROXY: 'httpProxy',
        HTTPS: 'httpsProxy',
        SOCKS: 'socks4Proxy',
        SOCKS4: 'socks4Proxy',
        SOCKS5: 'socks5Proxy'
      } as const
    )[protocol.toUpperCase()]
    if (kind) return { kind, host, port }
  }
  return null
}

let routingSession: Promise<Session> | undefined
function getRoutingSession(): Promise<Session> {
  return (routingSession ??= (async () => {
    const value = session.fromPartition('captain-who-network-routes', { cache: false })
    await value.setProxy({ mode: 'system' })
    return value
  })().catch((error: unknown) => {
    routingSession = undefined
    throw error
  }))
}

export async function resolveSystemNetworkRoute(url: string): Promise<NetworkRoute | null> {
  const network = await getRoutingSession()
  // Chromium observes system proxy changes. Resolve per request, including PAC's per-URL rules;
  // never freeze the result at launch or override it with inherited shell proxy variables.
  const result = await network.resolveProxy(url)
  return parseSystemProxyRoute(result)
}
