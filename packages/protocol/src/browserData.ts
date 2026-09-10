import {
  expectArray,
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectSchemaVersion,
  expectString,
  hasAsciiControlCharacter,
  invalidProtocolValue
} from './skills/validation'

export const BROWSER_DATA_SCHEMA_VERSION = 1 as const

export const BROWSER_LINK_OPEN_TARGETS = ['system', 'builtin'] as const
export type BrowserLinkOpenTarget = (typeof BROWSER_LINK_OPEN_TARGETS)[number]

export const BROWSER_DATA_TIME_RANGES = [
  'lastHour',
  'last24Hours',
  'last7Days',
  'last4Weeks',
  'allTime'
] as const
export type BrowserDataTimeRange = (typeof BROWSER_DATA_TIME_RANGES)[number]

export const BROWSER_DATA_CATEGORIES = [
  'history',
  'cookiesAndSiteData',
  'cache',
  'downloadHistory'
] as const
export type BrowserDataCategory = (typeof BROWSER_DATA_CATEGORIES)[number]

export interface BrowserPreferencesView {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  linkOpenTarget: BrowserLinkOpenTarget
  revision: number
  updatedAt: number
}

export interface BrowserPreferencesUpdateInput {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  linkOpenTarget: BrowserLinkOpenTarget
}

/** Main/Core-only optimistic-concurrency update. */
export interface BrowserPreferencesSaveInput extends BrowserPreferencesUpdateInput {
  expectedRevision: number
  updatedAt: number
}

export interface BrowserHistoryEntry {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  historyId: string
  url: string
  title: string
  hostname: string
  faviconUrl: string | null
  visitedAt: number
}

/** Main/Core-only insert. Main creates the identity before asynchronous persistence. */
export type BrowserHistoryRegistrationInput = BrowserHistoryEntry

/** Main/Core-only metadata update for the latest committed navigation. */
export interface BrowserHistoryMetadataUpdateInput {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  historyId: string
  title: string
  faviconUrl: string | null
}

export interface BrowserHistoryListInput {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  query: string
  limit: number
}

export interface BrowserHistoryListOutput {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  entries: BrowserHistoryEntry[]
  truncated: boolean
}

export interface BrowserHistoryDeleteInput {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  historyIds: string[]
}

export interface BrowserHistoryDeleteOutput {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  deletedCount: number
}

export interface BrowserHistoryChangedNotification {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
}

export interface BrowserOpenUrlInput {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  url: string
}

export interface BrowserDataSummaryInput {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  timeRange: BrowserDataTimeRange
}

export interface BrowserDataSummaryOutput {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  historyCount: number
  historySiteCount: number
  downloadCount: number
  cookieSiteCount: number
  cacheBytes: number
}

export interface BrowserDataClearInput extends BrowserDataSummaryInput {
  categories: BrowserDataCategory[]
}

export interface BrowserDataClearOutput {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  deletedHistoryCount: number
  deletedDownloadCount: number
  clearedCookiesAndSiteData: boolean
  clearedCache: boolean
}

/** Main/Core-only range input. `since` is inclusive and null means all time. */
export interface BrowserOwnedDataRangeInput {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  since: number | null
}

export interface BrowserOwnedDataSummary {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  historyCount: number
  historySiteCount: number
  downloadCount: number
}

/** Main/Core-only transaction request for app-owned history tables. */
export interface BrowserOwnedDataClearInput extends BrowserOwnedDataRangeInput {
  clearHistory: boolean
  clearDownloads: boolean
}

export interface BrowserOwnedDataClearOutput {
  schemaVersion: typeof BROWSER_DATA_SCHEMA_VERSION
  deletedHistoryCount: number
  deletedDownloadCount: number
}

const HISTORY_ID_PATTERN =
  /^browser-history:[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u

export function parseBrowserPreferencesView(
  value: unknown,
  context = 'Browser preferences'
): BrowserPreferencesView {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'linkOpenTarget', 'revision', 'updatedAt'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    linkOpenTarget: expectEnum(record.linkOpenTarget, BROWSER_LINK_OPEN_TARGETS, context),
    revision: expectSafeInteger(record.revision, `${context} revision`, 0),
    updatedAt: expectSafeInteger(record.updatedAt, `${context} updatedAt`, 0)
  }
}

export function parseBrowserPreferencesUpdateInput(
  value: unknown,
  context = 'Browser preferences update'
): BrowserPreferencesUpdateInput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'linkOpenTarget'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    linkOpenTarget: expectEnum(record.linkOpenTarget, BROWSER_LINK_OPEN_TARGETS, context)
  }
}

export function parseBrowserPreferencesSaveInput(
  value: unknown,
  context = 'Browser preferences save input'
): BrowserPreferencesSaveInput {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['schemaVersion', 'linkOpenTarget', 'expectedRevision', 'updatedAt'],
    context
  )
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    linkOpenTarget: expectEnum(record.linkOpenTarget, BROWSER_LINK_OPEN_TARGETS, context),
    expectedRevision: expectSafeInteger(record.expectedRevision, `${context} expectedRevision`, 0),
    updatedAt: expectSafeInteger(record.updatedAt, `${context} updatedAt`, 0)
  }
}

export function parseBrowserHistoryEntry(
  value: unknown,
  context = 'Browser history entry'
): BrowserHistoryEntry {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['schemaVersion', 'historyId', 'url', 'title', 'hostname', 'faviconUrl', 'visitedAt'],
    context
  )
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    historyId: parseHistoryId(record.historyId, context),
    url: parseHttpUrl(record.url, `${context} URL`, 8192),
    title: parseBoundedText(record.title, `${context} title`, 1024),
    hostname: parseHostname(record.hostname, context),
    faviconUrl: parseNullableHttpUrl(record.faviconUrl, `${context} favicon URL`, 4096),
    visitedAt: expectSafeInteger(record.visitedAt, `${context} visitedAt`, 0)
  }
}

export const parseBrowserHistoryRegistrationInput = parseBrowserHistoryEntry

export function parseBrowserHistoryMetadataUpdateInput(
  value: unknown,
  context = 'Browser history metadata update'
): BrowserHistoryMetadataUpdateInput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'historyId', 'title', 'faviconUrl'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    historyId: parseHistoryId(record.historyId, context),
    title: parseBoundedText(record.title, `${context} title`, 1024),
    faviconUrl: parseNullableHttpUrl(record.faviconUrl, `${context} favicon URL`, 4096)
  }
}

export function parseBrowserHistoryListInput(
  value: unknown,
  context = 'Browser history list input'
): BrowserHistoryListInput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'query', 'limit'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  const query = expectString(record.query, `${context} query`)
  if (query.length > 256 || hasAsciiControlCharacter(query)) {
    throw invalidProtocolValue(context, 'invalid query')
  }
  const limit = expectSafeInteger(record.limit, `${context} limit`, 1)
  if (limit > 500) throw invalidProtocolValue(context, 'limit exceeds 500')
  return { schemaVersion: BROWSER_DATA_SCHEMA_VERSION, query, limit }
}

export function parseBrowserHistoryListOutput(
  value: unknown,
  context = 'Browser history list output'
): BrowserHistoryListOutput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'entries', 'truncated'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    entries: expectArray(record.entries, `${context} entries`).map((entry) =>
      parseBrowserHistoryEntry(entry, `${context} entry`)
    ),
    truncated: expectBoolean(record.truncated, `${context} truncated`)
  }
}

export function parseBrowserHistoryDeleteInput(
  value: unknown,
  context = 'Browser history delete input'
): BrowserHistoryDeleteInput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'historyIds'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  const historyIds = expectArray(record.historyIds, `${context} historyIds`).map((id) =>
    parseHistoryId(id, context)
  )
  if (
    historyIds.length === 0 ||
    historyIds.length > 500 ||
    new Set(historyIds).size !== historyIds.length
  ) {
    throw invalidProtocolValue(context, 'historyIds must contain 1 to 500 unique identities')
  }
  return { schemaVersion: BROWSER_DATA_SCHEMA_VERSION, historyIds }
}

export function parseBrowserHistoryDeleteOutput(
  value: unknown,
  context = 'Browser history delete output'
): BrowserHistoryDeleteOutput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'deletedCount'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    deletedCount: expectSafeInteger(record.deletedCount, `${context} deletedCount`, 0)
  }
}

export function parseBrowserHistoryChangedNotification(
  value: unknown,
  context = 'Browser history changed notification'
): BrowserHistoryChangedNotification {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return { schemaVersion: BROWSER_DATA_SCHEMA_VERSION }
}

export function parseBrowserOpenUrlInput(
  value: unknown,
  context = 'Browser open URL input'
): BrowserOpenUrlInput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'url'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    url: parseHttpUrl(record.url, `${context} URL`, 8192)
  }
}

export function parseBrowserDataSummaryInput(
  value: unknown,
  context = 'Browser data summary input'
): BrowserDataSummaryInput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'timeRange'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    timeRange: expectEnum(record.timeRange, BROWSER_DATA_TIME_RANGES, context)
  }
}

export function parseBrowserDataSummaryOutput(
  value: unknown,
  context = 'Browser data summary output'
): BrowserDataSummaryOutput {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'historyCount',
      'historySiteCount',
      'downloadCount',
      'cookieSiteCount',
      'cacheBytes'
    ],
    context
  )
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    historyCount: expectSafeInteger(record.historyCount, `${context} historyCount`, 0),
    historySiteCount: expectSafeInteger(record.historySiteCount, `${context} historySiteCount`, 0),
    downloadCount: expectSafeInteger(record.downloadCount, `${context} downloadCount`, 0),
    cookieSiteCount: expectSafeInteger(record.cookieSiteCount, `${context} cookieSiteCount`, 0),
    cacheBytes: expectSafeInteger(record.cacheBytes, `${context} cacheBytes`, 0)
  }
}

export function parseBrowserDataClearInput(
  value: unknown,
  context = 'Browser data clear input'
): BrowserDataClearInput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'timeRange', 'categories'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  const timeRange = expectEnum(record.timeRange, BROWSER_DATA_TIME_RANGES, context)
  const categories = expectArray(record.categories, `${context} categories`).map((category) =>
    expectEnum(category, BROWSER_DATA_CATEGORIES, context)
  )
  if (categories.length === 0 || new Set(categories).size !== categories.length) {
    throw invalidProtocolValue(context, 'categories must contain unique values')
  }
  if (
    timeRange !== 'allTime' &&
    categories.some((category) => category === 'cookiesAndSiteData' || category === 'cache')
  ) {
    throw invalidProtocolValue(context, 'cookies and cache can only be cleared for all time')
  }
  return { schemaVersion: BROWSER_DATA_SCHEMA_VERSION, timeRange, categories }
}

export function parseBrowserDataClearOutput(
  value: unknown,
  context = 'Browser data clear output'
): BrowserDataClearOutput {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'deletedHistoryCount',
      'deletedDownloadCount',
      'clearedCookiesAndSiteData',
      'clearedCache'
    ],
    context
  )
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    deletedHistoryCount: expectSafeInteger(
      record.deletedHistoryCount,
      `${context} deletedHistoryCount`,
      0
    ),
    deletedDownloadCount: expectSafeInteger(
      record.deletedDownloadCount,
      `${context} deletedDownloadCount`,
      0
    ),
    clearedCookiesAndSiteData: expectBoolean(
      record.clearedCookiesAndSiteData,
      `${context} clearedCookiesAndSiteData`
    ),
    clearedCache: expectBoolean(record.clearedCache, `${context} clearedCache`)
  }
}

export function parseBrowserOwnedDataRangeInput(
  value: unknown,
  context = 'Browser owned data range input'
): BrowserOwnedDataRangeInput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'since'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    since: parseNullableTimestamp(record.since, `${context} since`)
  }
}

export function parseBrowserOwnedDataSummary(
  value: unknown,
  context = 'Browser owned data summary'
): BrowserOwnedDataSummary {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['schemaVersion', 'historyCount', 'historySiteCount', 'downloadCount'],
    context
  )
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    historyCount: expectSafeInteger(record.historyCount, `${context} historyCount`, 0),
    historySiteCount: expectSafeInteger(record.historySiteCount, `${context} historySiteCount`, 0),
    downloadCount: expectSafeInteger(record.downloadCount, `${context} downloadCount`, 0)
  }
}

export function parseBrowserOwnedDataClearInput(
  value: unknown,
  context = 'Browser owned data clear input'
): BrowserOwnedDataClearInput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'since', 'clearHistory', 'clearDownloads'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    since: parseNullableTimestamp(record.since, `${context} since`),
    clearHistory: expectBoolean(record.clearHistory, `${context} clearHistory`),
    clearDownloads: expectBoolean(record.clearDownloads, `${context} clearDownloads`)
  }
}

export function parseBrowserOwnedDataClearOutput(
  value: unknown,
  context = 'Browser owned data clear output'
): BrowserOwnedDataClearOutput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'deletedHistoryCount', 'deletedDownloadCount'], context)
  expectSchemaVersion(record, BROWSER_DATA_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    deletedHistoryCount: expectSafeInteger(
      record.deletedHistoryCount,
      `${context} deletedHistoryCount`,
      0
    ),
    deletedDownloadCount: expectSafeInteger(
      record.deletedDownloadCount,
      `${context} deletedDownloadCount`,
      0
    )
  }
}

function parseHistoryId(value: unknown, context: string): string {
  const historyId = expectString(value, `${context} historyId`)
  if (!HISTORY_ID_PATTERN.test(historyId)) {
    throw invalidProtocolValue(context, 'invalid history identity')
  }
  return historyId
}

function parseBoundedText(value: unknown, context: string, maximumBytes: number): string {
  const text = expectString(value, context)
  if (
    text.length === 0 ||
    new TextEncoder().encode(text).byteLength > maximumBytes ||
    hasAsciiControlCharacter(text)
  ) {
    throw invalidProtocolValue(context, 'invalid text')
  }
  return text
}

function parseHostname(value: unknown, context: string): string {
  const hostname = parseBoundedText(value, `${context} hostname`, 255).toLowerCase()
  if (hostname.includes('/') || hostname.includes('@')) {
    throw invalidProtocolValue(context, 'invalid hostname')
  }
  return hostname
}

function parseHttpUrl(value: unknown, context: string, maximumBytes: number): string {
  const text = parseBoundedText(value, context, maximumBytes)
  let url: URL
  try {
    url = new URL(text)
  } catch {
    throw invalidProtocolValue(context, 'invalid URL')
  }
  if (
    (url.protocol !== 'http:' && url.protocol !== 'https:' && url.protocol !== 'file:') ||
    url.username ||
    url.password
  ) {
    throw invalidProtocolValue(context, 'expected a credential-free HTTP(S) or file URL')
  }
  return url.toString()
}

function parseNullableHttpUrl(
  value: unknown,
  context: string,
  maximumBytes: number
): string | null {
  if (value === null) return null
  return parseHttpUrl(value, context, maximumBytes)
}

function parseNullableTimestamp(value: unknown, context: string): number | null {
  if (value === null) return null
  return expectSafeInteger(value, context, 0)
}
