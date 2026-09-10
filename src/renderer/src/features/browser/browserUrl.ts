const EXPLICIT_PROTOCOL_PATTERN = /^[a-zA-Z][a-zA-Z\d+.-]*:/
const HOST_WITH_PORT_PATTERN =
  /^(?:localhost|[^:/?#\s]+\.[^:/?#\s]+|\[[\da-f:.]+\]):\d+(?=[/?#]|$)/i
const LOCAL_HTTP_PATTERN = /^(localhost|127(?:\.\d{1,3}){3}|\[::1\])(?=[:/?#]|$)/i
const SAFE_BROWSER_PROTOCOLS = new Set(['http:', 'https:', 'file:'])

export function normalizeBrowserUrl(input: string): string | null {
  const trimmedInput = input.trim()
  if (!trimmedInput) return null

  // A dotted hostname or localhost followed by a port is an address, not a URL scheme.
  if (EXPLICIT_PROTOCOL_PATTERN.test(trimmedInput) && !HOST_WITH_PORT_PATTERN.test(trimmedInput)) {
    return normalizeHttpUrl(trimmedInput)
  }

  const candidate = LOCAL_HTTP_PATTERN.test(trimmedInput)
    ? `http://${trimmedInput}`
    : `https://${trimmedInput}`

  return normalizeHttpUrl(candidate)
}

export function getFallbackPageTitle(url: string | null): string | null {
  if (!url) return null

  try {
    const parsed = new URL(url)
    if (parsed.protocol === 'file:') return fileUrlDisplayName(parsed)
    return parsed.hostname.replace(/^www\./, '') || url
  } catch {
    return url
  }
}

export function fileUrlDisplayName(url: URL): string {
  const encoded = url.pathname.split('/').filter(Boolean).at(-1) ?? ''
  let name = encoded
  try {
    name = decodeURIComponent(encoded)
  } catch {
    // Keep the encoded segment when it is not valid UTF-8 percent-encoding.
  }
  const sanitized = name.replace(/[/\\@]/g, '-').trim()
  return sanitized || 'file'
}

function normalizeHttpUrl(input: string): string | null {
  try {
    const url = new URL(input)
    if (!SAFE_BROWSER_PROTOCOLS.has(url.protocol)) {
      return null
    }

    return url.href
  } catch {
    return null
  }
}
