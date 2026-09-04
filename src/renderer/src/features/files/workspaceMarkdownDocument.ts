import { parseDocument } from 'yaml'

const MAX_FRONT_MATTER_LENGTH = 64 * 1024
const MAX_METADATA_ENTRIES = 100
const MAX_METADATA_DEPTH = 5

export interface WorkspaceMarkdownMetadataEntry {
  key: string
  value: { kind: 'list'; items: string[] } | { kind: 'text'; text: string }
}

export type WorkspaceMarkdownMetadataError = 'invalid' | 'too-large'

export interface WorkspaceMarkdownDocument {
  body: string
  metadata: WorkspaceMarkdownMetadataEntry[]
  metadataError?: WorkspaceMarkdownMetadataError
  metadataTruncated: boolean
}

interface FrontMatterRange {
  bodyStart: number
  contentEnd: number
  contentStart: number
}

export function parseWorkspaceMarkdownDocument(source: string): WorkspaceMarkdownDocument {
  const frontMatterRange = findFrontMatterRange(source)
  if (!frontMatterRange) {
    return { body: source, metadata: [], metadataTruncated: false }
  }

  const body = source.slice(frontMatterRange.bodyStart)
  const frontMatter = source.slice(frontMatterRange.contentStart, frontMatterRange.contentEnd)
  if (frontMatter.length > MAX_FRONT_MATTER_LENGTH) {
    return { body, metadata: [], metadataError: 'too-large', metadataTruncated: false }
  }

  try {
    const document = parseDocument(frontMatter, {
      prettyErrors: false,
      uniqueKeys: true
    })
    if (document.errors.length > 0) {
      return { body, metadata: [], metadataError: 'invalid', metadataTruncated: false }
    }

    const value = document.toJS({ maxAliasCount: 20 }) as unknown
    if (value == null) return { body, metadata: [], metadataTruncated: false }
    if (!isPlainRecord(value)) {
      return { body, metadata: [], metadataError: 'invalid', metadataTruncated: false }
    }

    const metadata: WorkspaceMarkdownMetadataEntry[] = []
    const state = { truncated: false }
    for (const [key, entryValue] of Object.entries(value)) {
      appendMetadataEntry(metadata, key, entryValue, 0, state)
    }

    return { body, metadata, metadataTruncated: state.truncated }
  } catch {
    return { body, metadata: [], metadataError: 'invalid', metadataTruncated: false }
  }
}

function findFrontMatterRange(source: string): FrontMatterRange | null {
  const openingMatch = /^(?:\uFEFF)?---[\t ]*(?:\r\n|\n|\r)/.exec(source)
  if (!openingMatch) return null

  const contentStart = openingMatch[0].length
  let lineStart = contentStart
  while (lineStart <= source.length) {
    const lineBreakIndex = findLineBreakIndex(source, lineStart)
    const lineEnd = lineBreakIndex < 0 ? source.length : lineBreakIndex
    const line = source.slice(lineStart, lineEnd)

    if (/^(?:---|\.\.\.)[\t ]*$/.test(line)) {
      return {
        bodyStart: lineBreakIndex < 0 ? source.length : skipLineBreak(source, lineBreakIndex),
        contentEnd: lineStart,
        contentStart
      }
    }

    if (lineBreakIndex < 0) return null
    lineStart = skipLineBreak(source, lineBreakIndex)
  }

  return null
}

function findLineBreakIndex(source: string, fromIndex: number): number {
  for (let index = fromIndex; index < source.length; index += 1) {
    if (source[index] === '\n' || source[index] === '\r') return index
  }
  return -1
}

function skipLineBreak(source: string, index: number): number {
  return source[index] === '\r' && source[index + 1] === '\n' ? index + 2 : index + 1
}

function appendMetadataEntry(
  entries: WorkspaceMarkdownMetadataEntry[],
  key: string,
  value: unknown,
  depth: number,
  state: { truncated: boolean }
): void {
  if (entries.length >= MAX_METADATA_ENTRIES) {
    state.truncated = true
    return
  }

  if (isPlainRecord(value) && depth < MAX_METADATA_DEPTH) {
    const childEntries = Object.entries(value)
    if (childEntries.length === 0) {
      entries.push({ key, value: { kind: 'text', text: '{}' } })
      return
    }
    for (const [childKey, childValue] of childEntries) {
      appendMetadataEntry(entries, `${key}.${childKey}`, childValue, depth + 1, state)
    }
    return
  }

  if (Array.isArray(value) && value.every(isScalarMetadataValue)) {
    entries.push({
      key,
      value: { kind: 'list', items: value.map(formatScalarMetadataValue) }
    })
    return
  }

  entries.push({ key, value: { kind: 'text', text: formatMetadataValue(value) } })
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false
  const prototype = Object.getPrototypeOf(value) as object | null
  return prototype === Object.prototype || prototype === null
}

function isScalarMetadataValue(value: unknown): boolean {
  return value == null || ['boolean', 'number', 'string', 'bigint'].includes(typeof value)
}

function formatScalarMetadataValue(value: unknown): string {
  return value == null ? 'null' : String(value)
}

function formatMetadataValue(value: unknown): string {
  if (isScalarMetadataValue(value)) return formatScalarMetadataValue(value)
  if (value instanceof Date) return value.toISOString()
  try {
    return JSON.stringify(value) ?? String(value)
  } catch {
    return String(value)
  }
}
