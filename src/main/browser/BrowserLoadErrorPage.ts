import { randomBytes } from 'node:crypto'
import type {
  BrowserSurfaceLoadErrorKind,
  BrowserSurfacePublicLoadError
} from '@mycopilot/protocol'

const MAX_FAILED_URL_LENGTH = 16_384
const MAX_ERROR_DESCRIPTION_LENGTH = 128

export interface BrowserSurfaceLoadError extends BrowserSurfacePublicLoadError {
  internalPageUrl: string
  navigationEpoch: number
  generation: number
}

export interface BrowserSurfaceLoadErrorInput {
  errorCode: number
  errorDescription: string
  failedUrl: string
  generation: number
  locale?: string
  navigationEpoch: number
  nonce?: string
}

export function classifyBrowserLoadError(errorDescription: string): BrowserSurfaceLoadErrorKind {
  const description = normalizeErrorDescription(errorDescription)
  if (description === 'ERR_INTERNET_DISCONNECTED') return 'offline'
  if (description === 'ERR_NAME_NOT_RESOLVED' || description === 'DNS_PROBE_POSSIBLE') return 'dns'
  if (description === 'ERR_CONNECTION_REFUSED') return 'connection_refused'
  if (description === 'ERR_CONNECTION_TIMED_OUT' || description === 'ERR_TIMED_OUT') {
    return 'timeout'
  }
  if (description.startsWith('ERR_CERT_')) return 'certificate'
  return 'generic'
}

export function createBrowserSurfaceLoadError(
  input: BrowserSurfaceLoadErrorInput
): BrowserSurfaceLoadError {
  const failedUrl = normalizeFailedUrl(input.failedUrl)
  const errorDescription = normalizeErrorDescription(input.errorDescription)
  const kind = classifyBrowserLoadError(errorDescription)
  const hostname = new URL(failedUrl).hostname
  const copy = localizedErrorCopy(kind, input.locale)
  const publicError: BrowserSurfacePublicLoadError = {
    kind,
    errorCode: Number.isSafeInteger(input.errorCode) ? input.errorCode : -2,
    errorDescription,
    failedUrl,
    title: copy.title,
    heading: copy.heading,
    summary: copy.summary,
    suggestions: copy.suggestions
  }
  const internalPageUrl = createBrowserLoadErrorPageUrl(publicError, {
    hostname,
    nonce: input.nonce
  })
  return {
    ...publicError,
    internalPageUrl,
    navigationEpoch: input.navigationEpoch,
    generation: input.generation
  }
}

export function toPublicBrowserSurfaceLoadError(
  error: BrowserSurfaceLoadError
): BrowserSurfacePublicLoadError {
  return {
    kind: error.kind,
    errorCode: error.errorCode,
    errorDescription: error.errorDescription,
    failedUrl: error.failedUrl,
    title: error.title,
    heading: error.heading,
    summary: error.summary,
    suggestions: [...error.suggestions]
  }
}

export function createBrowserLoadErrorPageUrl(
  error: BrowserSurfacePublicLoadError,
  options: { hostname?: string; nonce?: string } = {}
): string {
  const hostname = options.hostname ?? new URL(error.failedUrl).hostname
  const nonce = normalizeNonce(options.nonce) ?? randomBytes(18).toString('base64url')
  const retryTarget = serializeInlineJson(error.failedUrl)
  const suggestionItems = error.suggestions
    .map((suggestion) => `<li>${escapeHtml(suggestion)}</li>`)
    .join('')
  const html = `<!doctype html>
<html lang="${isChineseLocale(error.title) ? 'zh-CN' : 'en'}">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; object-src 'none'; img-src data:; style-src 'nonce-${escapeHtmlAttribute(nonce)}'; script-src 'nonce-${escapeHtmlAttribute(nonce)}'">
  <title>${escapeHtml(error.title)}</title>
  <style nonce="${escapeHtmlAttribute(nonce)}">
    :root { color-scheme: light dark; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    * { box-sizing: border-box; }
    html, body { min-height: 100%; margin: 0; }
    body { display: grid; place-items: center; padding: clamp(24px, 8vw, 72px); background: #ffffff; color: #202124; }
    main { width: min(100%, 680px); }
    .mark { position: relative; width: 54px; height: 54px; margin-bottom: 28px; border: 4px solid #2878d0; border-radius: 50%; }
    .mark::before, .mark::after { content: ""; position: absolute; background: #2878d0; border-radius: 2px; transform: rotate(-38deg); }
    .mark::before { width: 4px; height: 28px; left: 23px; top: 9px; }
    .mark::after { width: 26px; height: 4px; left: 12px; top: 21px; }
    h1 { margin: 0; font-size: 32px; line-height: 1.25; font-weight: 650; letter-spacing: 0; }
    .summary { margin: 18px 0 0; color: #5f6368; font-size: 17px; line-height: 1.55; overflow-wrap: anywhere; }
    .host { color: #3c4043; font-weight: 600; }
    .try { margin: 30px 0 8px; color: #5f6368; font-size: 16px; }
    ul { margin: 0; padding-left: 27px; color: #5f6368; font-size: 16px; line-height: 1.7; }
    .code { margin: 28px 0 0; color: #70757a; font: 13px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace; overflow-wrap: anywhere; }
    button { margin-top: 34px; min-height: 40px; padding: 0 18px; border: 0; border-radius: 6px; background: #1a73e8; color: #fff; font: 600 14px/1 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; cursor: pointer; }
    button:focus-visible { outline: 3px solid color-mix(in srgb, #1a73e8 35%, transparent); outline-offset: 3px; }
    @media (max-width: 460px) {
      body { place-items: start; padding: 38px 24px; }
      .mark { width: 46px; height: 46px; margin-bottom: 22px; }
      .mark::before { height: 24px; left: 19px; top: 7px; }
      .mark::after { width: 22px; left: 10px; top: 18px; }
      h1 { font-size: 24px; }
      .summary, .try, ul { font-size: 15px; }
    }
    @media (prefers-color-scheme: dark) {
      body { background: #202124; color: #f1f3f4; }
      .summary, .try, ul { color: #bdc1c6; }
      .host { color: #e8eaed; }
      .code { color: #9aa0a6; }
      button { background: #8ab4f8; color: #202124; }
    }
  </style>
</head>
<body>
  <main>
    <div class="mark" aria-hidden="true"></div>
    <h1>${escapeHtml(error.heading)}</h1>
    <p class="summary"><span class="host">${escapeHtml(hostname)}</span> ${escapeHtml(error.summary)}</p>
    <p class="try">${escapeHtml(error.title.startsWith('无法') ? '请尝试：' : 'Try:')}</p>
    <ul>${suggestionItems}</ul>
    <p class="code">${escapeHtml(error.errorDescription)}</p>
    <button id="retry" type="button">${escapeHtml(error.title.startsWith('无法') ? '重新加载' : 'Reload')}</button>
  </main>
  <script nonce="${escapeHtmlAttribute(nonce)}">
    (() => {
      const target = ${retryTarget};
      document.getElementById('retry')?.addEventListener('click', () => window.location.replace(target));
    })();
  </script>
</body>
</html>`
  return `data:text/html;charset=utf-8,${encodeURIComponent(html)}`
}

function localizedErrorCopy(
  kind: BrowserSurfaceLoadErrorKind,
  locale?: string
): { title: string; heading: string; summary: string; suggestions: readonly string[] } {
  const chinese = locale?.toLowerCase().startsWith('zh') ?? true
  if (chinese) {
    const summary = {
      offline: '无法加载，因为设备处于离线状态。',
      dns: '的服务器 IP 地址无法找到。',
      connection_refused: '拒绝了连接。',
      timeout: '响应时间过长。',
      certificate: '使用了无效的安全证书。',
      generic: '暂时无法加载。'
    }[kind]
    return {
      title: '无法访问此网站',
      heading: '无法访问此网站',
      summary,
      suggestions:
        kind === 'certificate'
          ? ['检查设备日期和时间', '联系网站管理员']
          : ['检查网络连接', '检查代理、防火墙和 DNS 配置']
    }
  }
  const summary = {
    offline: 'could not be loaded because this device is offline.',
    dns: 'could not be found.',
    connection_refused: 'refused to connect.',
    timeout: 'took too long to respond.',
    certificate: 'presented an invalid security certificate.',
    generic: 'could not be loaded.'
  }[kind]
  return {
    title: 'This site cannot be reached',
    heading: 'This site cannot be reached',
    summary,
    suggestions:
      kind === 'certificate'
        ? ['Check your device date and time', 'Contact the site administrator']
        : ['Check the network connection', 'Check proxy, firewall, and DNS settings']
  }
}

function normalizeFailedUrl(value: string): string {
  if (value.length === 0 || value.length > MAX_FAILED_URL_LENGTH) {
    throw new Error('browser.invalid_failed_url')
  }
  let parsed: URL
  try {
    parsed = new URL(value)
  } catch {
    throw new Error('browser.invalid_failed_url')
  }
  if (
    !['http:', 'https:'].includes(parsed.protocol) ||
    parsed.username !== '' ||
    parsed.password !== ''
  ) {
    throw new Error('browser.invalid_failed_url')
  }
  return parsed.toString()
}

function normalizeErrorDescription(value: string): string {
  const normalized = value.trim().toUpperCase()
  const candidate =
    normalized.match(/(?:ERR_[A-Z0-9_]+|DNS_PROBE_POSSIBLE)/u)?.[0] ??
    normalized.replace(/[^A-Z0-9_]/gu, '').slice(0, MAX_ERROR_DESCRIPTION_LENGTH)
  return candidate.slice(0, MAX_ERROR_DESCRIPTION_LENGTH) || 'ERR_FAILED'
}

function normalizeNonce(value: string | undefined): string | null {
  return value && /^[A-Za-z0-9_-]{16,128}$/u.test(value) ? value : null
}

function escapeHtml(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;')
}

function escapeHtmlAttribute(value: string): string {
  return escapeHtml(value)
}

function serializeInlineJson(value: string): string {
  return JSON.stringify(value)
    .replaceAll('<', '\\u003c')
    .replaceAll('>', '\\u003e')
    .replaceAll('&', '\\u0026')
    .replaceAll('\u2028', '\\u2028')
    .replaceAll('\u2029', '\\u2029')
}

function isChineseLocale(title: string): boolean {
  return title.startsWith('无法')
}
