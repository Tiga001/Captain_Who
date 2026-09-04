export type WorkspaceMarkdownLinkTarget =
  | { kind: 'anchor'; anchor: string }
  | { kind: 'external'; url: string }
  | { kind: 'unsupported' }
  | { kind: 'workspace-file'; anchor?: string; path: string }

export function resolveWorkspaceMarkdownLink(
  currentPath: string,
  rawHref: string | undefined
): WorkspaceMarkdownLinkTarget {
  const href = rawHref?.trim()
  if (!href) return { kind: 'unsupported' }

  const externalUrl = normalizeExternalMarkdownUrl(href)
  if (externalUrl) return { kind: 'external', url: externalUrl }
  if (/^[a-z][a-z\d+.-]*:/i.test(href) || href.startsWith('//')) {
    return { kind: 'unsupported' }
  }

  const hashIndex = href.indexOf('#')
  const encodedAnchor = hashIndex >= 0 ? href.slice(hashIndex + 1) : undefined
  const hrefWithoutHash = hashIndex >= 0 ? href.slice(0, hashIndex) : href
  const queryIndex = hrefWithoutHash.indexOf('?')
  const encodedPath = queryIndex >= 0 ? hrefWithoutHash.slice(0, queryIndex) : hrefWithoutHash
  const anchor = decodeUriComponent(encodedAnchor)

  if (!encodedPath) {
    return anchor ? { anchor, kind: 'anchor' } : { kind: 'unsupported' }
  }

  const decodedPath = decodeUriComponent(encodedPath)
  if (!decodedPath || decodedPath.includes('\0')) return { kind: 'unsupported' }

  const fromWorkspaceRoot = decodedPath.startsWith('/')
  const normalizedHrefPath = decodedPath.replaceAll('\\', '/').replace(/^\/+/, '')
  const currentSegments = currentPath.split('/').filter(Boolean)
  const resolvedSegments = fromWorkspaceRoot ? [] : currentSegments.slice(0, -1)

  for (const segment of normalizedHrefPath.split('/')) {
    if (!segment || segment === '.') continue
    if (segment === '..') {
      if (resolvedSegments.length === 0) return { kind: 'unsupported' }
      resolvedSegments.pop()
      continue
    }
    resolvedSegments.push(segment)
  }

  if (resolvedSegments.length === 0) return { kind: 'unsupported' }
  return {
    ...(anchor ? { anchor } : {}),
    kind: 'workspace-file',
    path: resolvedSegments.join('/')
  }
}

export function normalizeExternalMarkdownUrl(href: string | undefined): string | null {
  if (!href) return null
  const normalizedHref = href.startsWith('//') ? `https:${href}` : href
  try {
    const url = new URL(normalizedHref)
    return url.protocol === 'http:' || url.protocol === 'https:' ? url.toString() : null
  } catch {
    return null
  }
}

function decodeUriComponent(value: string | undefined): string | undefined {
  if (value === undefined) return undefined
  try {
    return decodeURIComponent(value)
  } catch {
    return undefined
  }
}
