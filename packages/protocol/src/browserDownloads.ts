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

export const BROWSER_DOWNLOAD_SCHEMA_VERSION = 2 as const

export type BrowserDownloadSource = 'manual' | 'agent'
export type BrowserDownloadAvailability = 'available' | 'missing' | 'modified'
export type BrowserDownloadLocationMode = 'system' | 'custom'

/** Durable, path-free identity for a file saved by the managed browser. */
export interface BrowserDownloadReference {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
  downloadId: string
  displayName: string
  mimeType: string
  sizeBytes: number
  sha256: string
  createdAt: number
  source: BrowserDownloadSource
}

/** Renderer-safe settings. The Host keeps the actual custom directory private. */
export interface BrowserDownloadSettingsView {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
  locationMode: BrowserDownloadLocationMode
  displayPath: string
  askWhereToSave: boolean
  revision: number
  updatedAt: number
}

export interface BrowserDownloadAskWhereToSaveInput {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
  askWhereToSave: boolean
}

export interface BrowserDownloadHistoryItem extends BrowserDownloadReference {
  availability: BrowserDownloadAvailability
  sourceOrigin: string | null
}

export interface BrowserDownloadHistoryListInput {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
  query: string
  limit: number
}

export interface BrowserDownloadHistoryListOutput {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
  downloads: BrowserDownloadHistoryItem[]
  truncated: boolean
}

export interface BrowserDownloadIdInput {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
  downloadId: string
}

export interface BrowserDownloadHistoryClearInput {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
}

export interface BrowserDownloadHistoryClearOutput {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
  deletedCount: number
}

export interface BrowserDownloadRevealOutput {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
  status: 'shown' | 'missing'
}

export interface BrowserDownloadHistoryChangedNotification {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
}

/** Main/Core-only settings record. Never return this object to Renderer or model code. */
export interface BrowserDownloadSettingsRecord {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
  locationMode: BrowserDownloadLocationMode
  customDirectory: string | null
  askWhereToSave: boolean
  revision: number
  updatedAt: number
}

/** Main/Core-only registration request. `absolutePath` is stripped before public projection. */
export interface BrowserDownloadRegistrationInput {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
  downloadId: string
  source: BrowserDownloadSource
  displayName: string
  mimeType: string
  sizeBytes: number
  sha256: string
  absolutePath: string
  sourceOrigin: string | null
  conversationId: string | null
  runId: string | null
  callId: string | null
  createdAt: number
}

/** Main/Core-only history query. Renderer callers use BrowserDownloadHistoryListInput. */
export interface BrowserDownloadListInput {
  schemaVersion: typeof BROWSER_DOWNLOAD_SCHEMA_VERSION
  query: string
  limit: number
}

/** Main/Core-only durable record. Public callers must use `BrowserDownloadHistoryItem`. */
export interface BrowserDownloadRecord extends BrowserDownloadReference {
  absolutePath: string
  sourceOrigin: string | null
  conversationId: string | null
  projectId: string | null
  runId: string | null
  callId: string | null
}

const DOWNLOAD_ID_PATTERN =
  /^browser-download:[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u
const SHA256_PATTERN = /^[0-9a-f]{64}$/u
const MIME_TYPE_PATTERN = /^[a-z0-9][a-z0-9!#$&^_.+-]{0,63}\/[a-z0-9][a-z0-9!#$&^_.+-]{0,127}$/u
const SOURCES = ['manual', 'agent'] as const
const AVAILABILITIES = ['available', 'missing', 'modified'] as const
const LOCATION_MODES = ['system', 'custom'] as const

export function parseBrowserDownloadReference(
  value: unknown,
  context = 'Browser Download reference'
): BrowserDownloadReference {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'downloadId',
      'displayName',
      'mimeType',
      'sizeBytes',
      'sha256',
      'createdAt',
      'source'
    ],
    context
  )
  expectSchemaVersion(record, BROWSER_DOWNLOAD_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    downloadId: downloadId(record.downloadId, `${context}.downloadId`),
    displayName: displayName(record.displayName, `${context}.displayName`),
    mimeType: mimeType(record.mimeType, `${context}.mimeType`),
    sizeBytes: expectSafeInteger(record.sizeBytes, `${context}.sizeBytes`, 0),
    sha256: sha256(record.sha256, `${context}.sha256`),
    createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
    source: expectEnum(record.source, SOURCES, `${context}.source`)
  }
}

export function parseBrowserDownloadSettingsView(
  value: unknown,
  context = 'Browser Download settings view'
): BrowserDownloadSettingsView {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['schemaVersion', 'locationMode', 'displayPath', 'askWhereToSave', 'revision', 'updatedAt'],
    context
  )
  expectSchemaVersion(record, BROWSER_DOWNLOAD_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    locationMode: expectEnum(record.locationMode, LOCATION_MODES, `${context}.locationMode`),
    displayPath: safeText(record.displayPath, `${context}.displayPath`, 4096),
    askWhereToSave: expectBoolean(record.askWhereToSave, `${context}.askWhereToSave`),
    revision: expectSafeInteger(record.revision, `${context}.revision`, 0),
    updatedAt: expectSafeInteger(record.updatedAt, `${context}.updatedAt`, 0)
  }
}

export function parseBrowserDownloadAskWhereToSaveInput(
  value: unknown
): BrowserDownloadAskWhereToSaveInput {
  const context = 'Browser Download ask-where-to-save input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'askWhereToSave'], context)
  expectSchemaVersion(record, BROWSER_DOWNLOAD_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    askWhereToSave: expectBoolean(record.askWhereToSave, `${context}.askWhereToSave`)
  }
}

export function parseBrowserDownloadHistoryItem(
  value: unknown,
  context = 'Browser Download history item'
): BrowserDownloadHistoryItem {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'downloadId',
      'displayName',
      'mimeType',
      'sizeBytes',
      'sha256',
      'createdAt',
      'source',
      'availability',
      'sourceOrigin'
    ],
    context
  )
  const reference = parseBrowserDownloadReference(
    {
      schemaVersion: record.schemaVersion,
      downloadId: record.downloadId,
      displayName: record.displayName,
      mimeType: record.mimeType,
      sizeBytes: record.sizeBytes,
      sha256: record.sha256,
      createdAt: record.createdAt,
      source: record.source
    },
    context
  )
  return {
    ...reference,
    availability: expectEnum(record.availability, AVAILABILITIES, `${context}.availability`),
    sourceOrigin: nullableSafeText(record.sourceOrigin, `${context}.sourceOrigin`, 512)
  }
}

export function parseBrowserDownloadHistoryListInput(
  value: unknown
): BrowserDownloadHistoryListInput {
  const context = 'Browser Download history list input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'query', 'limit'], context)
  expectSchemaVersion(record, BROWSER_DOWNLOAD_SCHEMA_VERSION, context)
  const limit = expectSafeInteger(record.limit, `${context}.limit`, 1)
  if (limit > 500) throw invalidProtocolValue(`${context}.limit`, 'exceeds 500')
  return {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    query: safeText(record.query, `${context}.query`, 256, true),
    limit
  }
}

export function parseBrowserDownloadHistoryListOutput(
  value: unknown
): BrowserDownloadHistoryListOutput {
  const context = 'Browser Download history list output'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'downloads', 'truncated'], context)
  expectSchemaVersion(record, BROWSER_DOWNLOAD_SCHEMA_VERSION, context)
  const downloads = expectArray(record.downloads, `${context}.downloads`)
  if (downloads.length > 500) throw invalidProtocolValue(`${context}.downloads`, 'exceeds 500')
  if (typeof record.truncated !== 'boolean') {
    throw invalidProtocolValue(`${context}.truncated`, 'expected a boolean')
  }
  return {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    downloads: downloads.map((item, index) =>
      parseBrowserDownloadHistoryItem(item, `${context}.downloads[${index}]`)
    ),
    truncated: record.truncated
  }
}

export function parseBrowserDownloadIdInput(value: unknown): BrowserDownloadIdInput {
  const context = 'Browser Download id input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'downloadId'], context)
  expectSchemaVersion(record, BROWSER_DOWNLOAD_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    downloadId: downloadId(record.downloadId, `${context}.downloadId`)
  }
}

export function parseBrowserDownloadHistoryClearInput(
  value: unknown
): BrowserDownloadHistoryClearInput {
  const context = 'Browser Download history clear input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion'], context)
  expectSchemaVersion(record, BROWSER_DOWNLOAD_SCHEMA_VERSION, context)
  return { schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION }
}

export function parseBrowserDownloadHistoryClearOutput(
  value: unknown
): BrowserDownloadHistoryClearOutput {
  const context = 'Browser Download history clear output'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'deletedCount'], context)
  expectSchemaVersion(record, BROWSER_DOWNLOAD_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    deletedCount: expectSafeInteger(record.deletedCount, `${context}.deletedCount`, 0)
  }
}

export function parseBrowserDownloadRevealOutput(value: unknown): BrowserDownloadRevealOutput {
  const context = 'Browser Download reveal output'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'status'], context)
  expectSchemaVersion(record, BROWSER_DOWNLOAD_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    status: expectEnum(record.status, ['shown', 'missing'] as const, `${context}.status`)
  }
}

export function parseBrowserDownloadHistoryChangedNotification(
  value: unknown
): BrowserDownloadHistoryChangedNotification {
  const context = 'Browser Download history changed notification'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion'], context)
  expectSchemaVersion(record, BROWSER_DOWNLOAD_SCHEMA_VERSION, context)
  return { schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION }
}

export function parseBrowserDownloadSettingsRecord(value: unknown): BrowserDownloadSettingsRecord {
  const context = 'Browser Download settings record'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['schemaVersion', 'locationMode', 'customDirectory', 'askWhereToSave', 'revision', 'updatedAt'],
    context
  )
  expectSchemaVersion(record, BROWSER_DOWNLOAD_SCHEMA_VERSION, context)
  const locationMode = expectEnum(record.locationMode, LOCATION_MODES, `${context}.locationMode`)
  const customDirectory = nullableSafeText(
    record.customDirectory,
    `${context}.customDirectory`,
    4096
  )
  if ((locationMode === 'custom') !== (customDirectory !== null)) {
    throw invalidProtocolValue(context, 'custom location requires exactly one custom directory')
  }
  return {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    locationMode,
    customDirectory,
    askWhereToSave: expectBoolean(record.askWhereToSave, `${context}.askWhereToSave`),
    revision: expectSafeInteger(record.revision, `${context}.revision`, 0),
    updatedAt: expectSafeInteger(record.updatedAt, `${context}.updatedAt`, 0)
  }
}

export function parseBrowserDownloadRegistrationInput(
  value: unknown
): BrowserDownloadRegistrationInput {
  const context = 'Browser Download registration input'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'downloadId',
      'source',
      'displayName',
      'mimeType',
      'sizeBytes',
      'sha256',
      'absolutePath',
      'sourceOrigin',
      'conversationId',
      'runId',
      'callId',
      'createdAt'
    ],
    context
  )
  expectSchemaVersion(record, BROWSER_DOWNLOAD_SCHEMA_VERSION, context)
  const source = expectEnum(record.source, SOURCES, `${context}.source`)
  const conversationId = nullableSafeText(record.conversationId, `${context}.conversationId`, 256)
  const runId = nullableSafeText(record.runId, `${context}.runId`, 256)
  const callId = nullableSafeText(record.callId, `${context}.callId`, 256)
  if (source === 'agent' && (!conversationId || !runId || !callId)) {
    throw invalidProtocolValue(context, 'Agent downloads require conversation, run and call ids')
  }
  if (source === 'manual' && (conversationId || runId || callId)) {
    throw invalidProtocolValue(context, 'manual downloads cannot claim Agent ownership')
  }
  return {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    downloadId: downloadId(record.downloadId, `${context}.downloadId`),
    source,
    displayName: displayName(record.displayName, `${context}.displayName`),
    mimeType: mimeType(record.mimeType, `${context}.mimeType`),
    sizeBytes: expectSafeInteger(record.sizeBytes, `${context}.sizeBytes`, 0),
    sha256: sha256(record.sha256, `${context}.sha256`),
    absolutePath: safeText(record.absolutePath, `${context}.absolutePath`, 4096),
    sourceOrigin: nullableSafeText(record.sourceOrigin, `${context}.sourceOrigin`, 512),
    conversationId,
    runId,
    callId,
    createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  }
}

export function parseBrowserDownloadRecord(value: unknown): BrowserDownloadRecord {
  const context = 'Browser Download record'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'downloadId',
      'displayName',
      'mimeType',
      'sizeBytes',
      'sha256',
      'createdAt',
      'source',
      'absolutePath',
      'sourceOrigin',
      'conversationId',
      'projectId',
      'runId',
      'callId'
    ],
    context
  )
  const reference = parseBrowserDownloadReference(
    {
      schemaVersion: record.schemaVersion,
      downloadId: record.downloadId,
      displayName: record.displayName,
      mimeType: record.mimeType,
      sizeBytes: record.sizeBytes,
      sha256: record.sha256,
      createdAt: record.createdAt,
      source: record.source
    },
    context
  )
  return {
    ...reference,
    absolutePath: safeText(record.absolutePath, `${context}.absolutePath`, 4096),
    sourceOrigin: nullableSafeText(record.sourceOrigin, `${context}.sourceOrigin`, 512),
    conversationId: nullableSafeText(record.conversationId, `${context}.conversationId`, 256),
    projectId: nullableSafeText(record.projectId, `${context}.projectId`, 256),
    runId: nullableSafeText(record.runId, `${context}.runId`, 256),
    callId: nullableSafeText(record.callId, `${context}.callId`, 256)
  }
}

function downloadId(value: unknown, context: string): string {
  const id = expectString(value, context)
  if (!DOWNLOAD_ID_PATTERN.test(id)) throw invalidProtocolValue(context, 'invalid id')
  return id
}

function sha256(value: unknown, context: string): string {
  const digest = expectString(value, context)
  if (!SHA256_PATTERN.test(digest)) throw invalidProtocolValue(context, 'invalid SHA-256')
  return digest
}

function mimeType(value: unknown, context: string): string {
  const mime = expectString(value, context)
  if (!MIME_TYPE_PATTERN.test(mime)) throw invalidProtocolValue(context, 'invalid MIME type')
  return mime
}

function displayName(value: unknown, context: string): string {
  const name = safeText(value, context, 255)
  const canonical = name.normalize('NFKC').trim()
  if (
    name !== canonical ||
    name === '.' ||
    name === '..' ||
    name.startsWith('.') ||
    name.endsWith('.') ||
    /[<>:"/\\|?*]/u.test(name)
  ) {
    throw invalidProtocolValue(context, 'must be a canonical path-free file name')
  }
  return name
}

function safeText(value: unknown, context: string, maximum: number, allowEmpty = false): string {
  const text = expectString(value, context)
  if (
    (!allowEmpty && text.trim().length === 0) ||
    text.length > maximum ||
    hasAsciiControlCharacter(text)
  ) {
    throw invalidProtocolValue(context, 'invalid text')
  }
  return text
}

function nullableSafeText(value: unknown, context: string, maximum: number): string | null {
  if (value === null) return null
  return safeText(value, context, maximum)
}
