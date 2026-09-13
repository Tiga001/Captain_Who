import { parse } from 'yaml'

/** Read the generated, app-signed configuration. Never read a runtime environment override. */
export function parseUpdateSource(contents: string): URL {
  if (contents.length > 16_384) throw new Error('Invalid update configuration')
  const config = parse(contents) as Record<string, unknown> | null
  if (
    !config ||
    typeof config !== 'object' ||
    Array.isArray(config) ||
    config.provider !== 'generic' ||
    typeof config.url !== 'string' ||
    Object.keys(config).some(
      (key) =>
        !['provider', 'url', 'channel', 'updaterCacheDirName', 'useMultipleRangeRequest'].includes(
          key
        )
    ) ||
    (config.channel !== undefined && config.channel !== 'latest')
  )
    throw new Error('Invalid update configuration')
  const source = new URL(config.url)
  if (
    source.protocol !== 'https:' ||
    source.username ||
    source.password ||
    source.search ||
    source.hash
  )
    throw new Error('Invalid update source')
  if (!source.pathname.endsWith('/')) source.pathname += '/'
  return source
}

export function isAllowedUpdateRequest(rawUrl: string, source: URL): boolean {
  try {
    const url = new URL(rawUrl)
    return (
      url.protocol === 'https:' &&
      url.origin === source.origin &&
      url.pathname.startsWith(source.pathname) &&
      !url.username &&
      !url.password &&
      !url.hash
    )
  } catch {
    return false
  }
}

/** Generic COS delivery needs no account credentials or stable rollout/device identifiers. */
export function stripUpdateIdentityHeaders(
  headers: Record<string, string>
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(headers).filter(
      ([name]) =>
        ![
          'authorization',
          'proxy-authorization',
          'cookie',
          'referer',
          'x-user-staging-id'
        ].includes(name.toLowerCase())
    )
  )
}
