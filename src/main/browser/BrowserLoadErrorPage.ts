import { randomBytes } from 'node:crypto'
import type {
  BrowserSurfaceCrashErrorKind,
  BrowserSurfaceLoadErrorKind,
  BrowserSurfacePublicCrashError,
  BrowserSurfacePublicLoadError
} from '@mycopilot/protocol'
import {
  DEFAULT_APP_LANGUAGE,
  getTranslation,
  isAppLanguage,
  type AppLanguage,
  type TranslationKey
} from '../../shared/i18n/languageRegistry'
import { BROWSER_INTERNAL_PAGE_SCHEME } from './BrowserInternalPageStore'

const MAX_FAILED_URL_LENGTH = 16_384
const MAX_ERROR_DESCRIPTION_LENGTH = 128
const INTERNAL_ACTION_ORIGIN = `${BROWSER_INTERNAL_PAGE_SCHEME}://action`

export interface BrowserSurfaceLoadError extends BrowserSurfacePublicLoadError {
  generation: number
  internalActionUrl: string
  internalPageHtml: string
  navigationEpoch: number
}

export interface BrowserSurfaceCrashError extends BrowserSurfacePublicCrashError {
  generation: number
  internalActionUrl: string
  internalPageHtml: string
  navigationEpoch: number
}

export interface BrowserSurfaceLoadErrorInput {
  actionToken?: string
  errorCode: number
  errorDescription: string
  failedUrl: string
  generation: number
  locale?: string
  navigationEpoch: number
  nonce?: string
}

export interface BrowserSurfaceCrashErrorInput {
  actionToken?: string
  generation: number
  kind: BrowserSurfaceCrashErrorKind
  locale?: string
  logicalUrl: string | null
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
  const locale = normalizeLocale(input.locale)
  const hostname = new URL(failedUrl).hostname
  const copy = localizedLoadErrorCopy(kind, hostname, locale)
  const internalActionUrl = createBrowserInternalActionUrl('retry', input.actionToken)
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
  return {
    ...publicError,
    generation: input.generation,
    internalActionUrl,
    internalPageHtml: createBrowserLoadErrorPageHtml(publicError, {
      actionUrl: internalActionUrl,
      locale,
      nonce: input.nonce
    }),
    navigationEpoch: input.navigationEpoch
  }
}

export function createBrowserSurfaceCrashError(
  input: BrowserSurfaceCrashErrorInput
): BrowserSurfaceCrashError {
  const locale = normalizeLocale(input.locale)
  const hostname = hostnameForLogicalUrl(input.logicalUrl)
  const titleKey =
    input.kind === 'renderer_unresponsive'
      ? 'browser.internalUnresponsive.title'
      : 'browser.internalCrash.title'
  const summaryKey =
    input.kind === 'renderer_unresponsive'
      ? 'browser.internalUnresponsive.summary'
      : 'browser.internalCrash.summary'
  const actionKey =
    input.kind === 'renderer_unresponsive'
      ? 'browser.internalUnresponsive.action'
      : 'browser.internalCrash.action'
  const internalActionUrl = createBrowserInternalActionUrl('recover', input.actionToken)
  const publicError: BrowserSurfacePublicCrashError = {
    kind: input.kind,
    title: translate(locale, titleKey),
    heading: translate(locale, titleKey),
    summary: translate(locale, summaryKey, { host: hostname }),
    actionLabel: translate(locale, actionKey)
  }
  return {
    ...publicError,
    generation: input.generation,
    internalActionUrl,
    internalPageHtml: createBrowserCrashPageHtml(publicError, {
      actionUrl: internalActionUrl,
      locale,
      nonce: input.nonce
    }),
    navigationEpoch: input.navigationEpoch
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

export function toPublicBrowserSurfaceCrashError(
  error: BrowserSurfaceCrashError
): BrowserSurfacePublicCrashError {
  return {
    kind: error.kind,
    title: error.title,
    heading: error.heading,
    summary: error.summary,
    actionLabel: error.actionLabel
  }
}

export function createBrowserLoadErrorPageHtml(
  error: BrowserSurfacePublicLoadError,
  options: { actionUrl?: string; locale?: string; nonce?: string } = {}
): string {
  const locale = normalizeLocale(options.locale)
  const actionUrl = normalizeInternalActionUrl(
    options.actionUrl ?? createBrowserInternalActionUrl('retry')
  )
  return createInternalPageHtml({
    actionLabel: translate(locale, 'browser.internalError.reload'),
    actionUrl,
    code: error.errorDescription,
    heading: error.heading,
    locale,
    nonce: options.nonce,
    summary: error.summary,
    suggestions: error.suggestions,
    title: error.title,
    tryLabel: translate(locale, 'browser.internalError.try')
  })
}

export function createBrowserCrashPageHtml(
  error: BrowserSurfacePublicCrashError,
  options: { actionUrl?: string; locale?: string; nonce?: string } = {}
): string {
  const locale = normalizeLocale(options.locale)
  const actionUrl = normalizeInternalActionUrl(
    options.actionUrl ?? createBrowserInternalActionUrl('recover')
  )
  return createInternalPageHtml({
    actionLabel: error.actionLabel,
    actionUrl,
    heading: error.heading,
    locale,
    nonce: options.nonce,
    summary: error.summary,
    suggestions: [],
    title: error.title
  })
}

export function isBrowserInternalActionUrl(value: string): boolean {
  try {
    const parsed = new URL(value)
    return (
      parsed.protocol === `${BROWSER_INTERNAL_PAGE_SCHEME}:` &&
      parsed.hostname === 'action' &&
      /^\/(?:retry|recover)\/[A-Za-z0-9_-]{24,128}$/u.test(parsed.pathname) &&
      parsed.port === '' &&
      parsed.username === '' &&
      parsed.password === '' &&
      parsed.search === '' &&
      parsed.hash === ''
    )
  } catch {
    return false
  }
}

function createInternalPageHtml(input: {
  actionLabel: string
  actionUrl: string
  code?: string
  heading: string
  locale: AppLanguage
  nonce?: string
  summary: string
  suggestions: readonly string[]
  title: string
  tryLabel?: string
}): string {
  const nonce = normalizeNonce(input.nonce) ?? randomBytes(18).toString('base64url')
  const actionTarget = serializeInlineJson(input.actionUrl)
  const suggestionBlock =
    input.suggestions.length > 0
      ? `<section class="suggestions" aria-label="${escapeHtmlAttribute(input.tryLabel ?? '')}">
      <p>${escapeHtml(input.tryLabel ?? '')}</p>
      <ul>${input.suggestions.map((suggestion) => `<li>${escapeHtml(suggestion)}</li>`).join('')}</ul>
    </section>`
      : ''
  const code = input.code ? `<p class="code">${escapeHtml(input.code)}</p>` : ''
  const html = `<!doctype html>
<html lang="${escapeHtmlAttribute(input.locale)}" dir="ltr">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; base-uri 'none'; connect-src 'none'; form-action 'none'; frame-ancestors 'none'; frame-src 'none'; object-src 'none'; img-src 'none'; media-src 'none'; style-src 'nonce-${escapeHtmlAttribute(nonce)}'; script-src 'nonce-${escapeHtmlAttribute(nonce)}'">
  <title>${escapeHtml(input.title)}</title>
  <style nonce="${escapeHtmlAttribute(nonce)}">
    :root {
      color-scheme: light dark;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      --page: #ffffff;
      --text: #202124;
      --muted: #62666d;
      --accent: #1673d1;
      --button: #eef1f4;
      --button-hover: #e2e6ea;
      --focus: #1673d1;
    }
    * { box-sizing: border-box; }
    html, body { min-height: 100%; margin: 0; }
    body { background: var(--page); color: var(--text); }
    main {
      display: flex;
      min-height: 100vh;
      align-items: center;
      justify-content: center;
      padding: clamp(32px, 7vw, 76px);
    }
    .content { width: min(100%, 680px); min-width: 0; }
    .mark {
      position: relative;
      width: 52px;
      height: 52px;
      margin-bottom: 28px;
      color: var(--accent);
      border: 4px solid currentColor;
      border-radius: 50%;
    }
    .mark::before, .mark::after {
      position: absolute;
      content: "";
      background: currentColor;
      border-radius: 2px;
      transform: rotate(-38deg);
    }
    .mark::before { width: 4px; height: 28px; left: 20px; top: 8px; }
    .mark::after { width: 25px; height: 4px; left: 10px; top: 20px; }
    h1 { margin: 0; font-size: 30px; font-weight: 650; line-height: 1.28; letter-spacing: 0; }
    .summary { margin: 18px 0 0; color: var(--muted); font-size: 17px; line-height: 1.55; overflow-wrap: anywhere; }
    .suggestions { margin-top: 30px; color: var(--muted); }
    .suggestions p { margin: 0 0 8px; font-size: 16px; }
    ul { margin: 0; padding-left: 26px; font-size: 16px; line-height: 1.72; }
    .code { margin: 28px 0 0; color: var(--muted); font: 13px/1.45 ui-monospace, SFMono-Regular, Menlo, monospace; overflow-wrap: anywhere; }
    button {
      min-height: 40px;
      margin-top: 34px;
      padding: 0 17px;
      color: var(--text);
      background: var(--button);
      border: 0;
      border-radius: 7px;
      font: 600 14px/1 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      cursor: pointer;
    }
    button:hover { background: var(--button-hover); }
    button:focus-visible { outline: 3px solid color-mix(in srgb, var(--focus) 40%, transparent); outline-offset: 3px; }
    @media (max-width: 460px) {
      main { align-items: flex-start; justify-content: flex-start; padding: 38px 24px; }
      .mark { width: 46px; height: 46px; margin-bottom: 22px; }
      .mark::before { height: 24px; left: 18px; top: 7px; }
      .mark::after { width: 22px; left: 9px; top: 18px; }
      h1 { font-size: 24px; }
      .summary, .suggestions p, ul { font-size: 15px; }
    }
    @media (prefers-color-scheme: dark) {
      :root { --page: #202428; --text: #f1f3f4; --muted: #b7bdc5; --accent: #6aa9e9; --button: #343a40; --button-hover: #40474e; --focus: #8abcf0; }
    }
    @media (prefers-reduced-motion: reduce) { * { scroll-behavior: auto !important; } }
  </style>
</head>
<body>
  <main>
    <div class="content">
      <div class="mark" aria-hidden="true"></div>
      <h1>${escapeHtml(input.heading)}</h1>
      <p class="summary">${escapeHtml(input.summary)}</p>
      ${suggestionBlock}
      ${code}
      <button id="primary-action" type="button">${escapeHtml(input.actionLabel)}</button>
    </div>
  </main>
  <script nonce="${escapeHtmlAttribute(nonce)}">
    (() => {
      const target = ${actionTarget};
      document.getElementById('primary-action')?.addEventListener('click', () => {
        document.title = target;
      });
    })();
  </script>
</body>
</html>`
  return html
}

function localizedLoadErrorCopy(
  kind: BrowserSurfaceLoadErrorKind,
  hostname: string,
  locale: AppLanguage
): { title: string; heading: string; summary: string; suggestions: readonly string[] } {
  const summaryKey: Record<BrowserSurfaceLoadErrorKind, TranslationKey> = {
    offline: 'browser.internalError.offlineSummary',
    dns: 'browser.internalError.dnsSummary',
    connection_refused: 'browser.internalError.refusedSummary',
    timeout: 'browser.internalError.timeoutSummary',
    certificate: 'browser.internalError.certificateSummary',
    generic: 'browser.internalError.genericSummary'
  }
  const title = translate(locale, 'browser.internalError.title')
  return {
    title,
    heading: title,
    summary: translate(locale, summaryKey[kind], { host: hostname }),
    suggestions:
      kind === 'certificate'
        ? [
            translate(locale, 'browser.internalError.checkDateTime'),
            translate(locale, 'browser.internalError.contactAdministrator')
          ]
        : [
            translate(locale, 'browser.internalError.checkNetwork'),
            translate(locale, 'browser.internalError.checkProxyFirewallDns')
          ]
  }
}

function createBrowserInternalActionUrl(action: 'recover' | 'retry', token?: string): string {
  const normalizedToken = normalizeActionToken(token) ?? randomBytes(24).toString('base64url')
  return `${INTERNAL_ACTION_ORIGIN}/${action}/${normalizedToken}`
}

function normalizeInternalActionUrl(value: string): string {
  if (!isBrowserInternalActionUrl(value)) throw new Error('browser.invalid_internal_action')
  return value
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

function normalizeLocale(value: string | undefined): AppLanguage {
  return isAppLanguage(value) ? value : DEFAULT_APP_LANGUAGE
}

function normalizeNonce(value: string | undefined): string | null {
  return value && /^[A-Za-z0-9_-]{16,128}$/u.test(value) ? value : null
}

function normalizeActionToken(value: string | undefined): string | null {
  return value && /^[A-Za-z0-9_-]{24,128}$/u.test(value) ? value : null
}

function hostnameForLogicalUrl(value: string | null): string {
  if (!value) return 'page'
  try {
    const parsed = new URL(value)
    return ['http:', 'https:'].includes(parsed.protocol) ? parsed.hostname : 'page'
  } catch {
    return 'page'
  }
}

function translate(
  locale: AppLanguage,
  key: TranslationKey,
  values: Readonly<Record<string, string>> = {}
): string {
  return getTranslation(locale, key).replace(/\{([^{}]+)\}/gu, (placeholder, name: string) =>
    Object.prototype.hasOwnProperty.call(values, name) ? values[name] : placeholder
  )
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
