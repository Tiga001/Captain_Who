import { isUtf8 } from 'node:buffer'
import { lstat, open, readdir, realpath } from 'node:fs/promises'
import { extname, isAbsolute, join, relative, resolve, win32 } from 'node:path'
import type {
  AgentWorkspaceContext,
  StorageProjectRecord,
  WorkspaceDirectoryEntry,
  WorkspaceDirectoryEntryKind,
  WorkspaceDirectoryListing,
  WorkspaceFileMetadata,
  WorkspaceFilePreviewResult,
  WorkspaceFileRequest,
  WorkspaceListDirectoryInput,
  WorkspaceMentionSearchInput,
  WorkspaceMentionSearchResult,
  WorkspaceMentionSearchEntry
} from '@mycopilot/protocol'

const DIRECTORY_ENTRY_LIMIT = 20_000
const IMAGE_PREVIEW_LIMIT_BYTES = 12 * 1024 * 1024
export const PDF_PREVIEW_LIMIT_BYTES = 32 * 1024 * 1024
const TEXT_PREVIEW_LIMIT_BYTES = 1024 * 1024
const PDF_HEADER = Buffer.from('%PDF-')
const PDF_HEADER_SEARCH_BYTES = 1024
const MENTION_SEARCH_DEFAULT_LIMIT = 40
const MENTION_SEARCH_MAX_LIMIT = 100
const MENTION_SEARCH_MAX_QUERY_LENGTH = 256
const MENTION_SEARCH_MAX_VISITED = 50_000
// Keep enough candidates to rank a query across all roots.  Stopping as soon as the
// requested page is full would make a shallow, unrelated root hide a better match in a
// later root.
const MENTION_SEARCH_MAX_MATCHES = 20_000
const MENTION_SEARCH_IGNORED_DIRECTORIES = new Set([
  '.git',
  '.cache',
  'node_modules',
  'build',
  'dist',
  'out',
  'target'
])

const IMAGE_MIME_BY_EXTENSION: Readonly<Record<string, string>> = {
  '.avif': 'image/avif',
  '.bmp': 'image/bmp',
  '.gif': 'image/gif',
  '.jpeg': 'image/jpeg',
  '.jpg': 'image/jpeg',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.webp': 'image/webp'
}

export type WorkspaceProjectResolver = (
  projectId: string
) => Promise<Pick<StorageProjectRecord, 'id' | 'folders'> | null | undefined>

interface WorkspaceRunFileSource {
  loadRunWorkspace(input: {
    assistantMessageId: string
    projectId: string
  }): Promise<AgentWorkspaceContext | null>
  resolveRunWorkspacePath(input: {
    assistantMessageId: string
    projectId: string
    filePath: string
  }): Promise<string>
}

interface ResolvedWorkspaceEntry {
  kind: WorkspaceDirectoryEntryKind
  modifiedAtMs: number
  path: string
  realPath: string | null
  sizeBytes: number
}

export class WorkspaceFilesService {
  constructor(
    private readonly resolveProject: WorkspaceProjectResolver,
    private readonly runFiles?: WorkspaceRunFileSource
  ) {}

  /**
   * Searches configured project roots for @ references. The result deliberately contains no
   * native paths; the Host resolves the selected alias/path again when it is used.
   */
  async searchMentions(input: WorkspaceMentionSearchInput): Promise<WorkspaceMentionSearchResult> {
    const projectId = requireProjectId(input?.projectId)
    const query = normalizeMentionQuery(input?.query)
    if (!query) return { entries: [], truncated: false }
    const project = await this.resolveProject(projectId)
    if (!project || project.id !== projectId) throw new Error('Project is not available')

    const limit = clampMentionLimit(input?.limit)
    const candidates: Array<WorkspaceMentionSearchEntry & { score: number }> = []
    let visited = 0
    let truncated = false
    const folders = [...project.folders].sort(
      (left, right) => left.sortOrder - right.sortOrder || left.alias.localeCompare(right.alias)
    )

    for (const folder of folders) {
      if (visited >= MENTION_SEARCH_MAX_VISITED) {
        truncated = true
        break
      }
      let root: string
      try {
        root = await realpath(resolve(folder.path))
        const rootStat = await lstat(root)
        if (!rootStat.isDirectory()) continue
      } catch {
        continue
      }
      const queue: Array<{ absolute: string; path: string }> = [{ absolute: root, path: '' }]
      for (let queueIndex = 0; queueIndex < queue.length; queueIndex += 1) {
        const current = queue[queueIndex]
        let entries
        try {
          entries = await readdir(current.absolute, { withFileTypes: true })
        } catch {
          continue
        }
        for (const entry of entries) {
          visited += 1
          if (visited >= MENTION_SEARCH_MAX_VISITED) {
            truncated = true
            break
          }
          if (entry.isSymbolicLink()) continue
          const childPath = current.path ? `${current.path}/${entry.name}` : entry.name
          const isDirectory = entry.isDirectory()
          if (isDirectory && MENTION_SEARCH_IGNORED_DIRECTORIES.has(entry.name)) continue
          const score = mentionMatchScore(query, childPath)
          if (score !== null && candidates.length < MENTION_SEARCH_MAX_MATCHES) {
            candidates.push({
              alias: folder.alias,
              displayName: entry.name,
              folderId: folder.id,
              kind: isDirectory ? 'directory' : 'file',
              path: childPath,
              displayPath: `${folder.alias}/${childPath}`,
              score
            })
          }
          if (isDirectory)
            queue.push({ absolute: join(current.absolute, entry.name), path: childPath })
        }
        if (truncated) break
      }
      if (truncated) break
    }

    if (candidates.length >= MENTION_SEARCH_MAX_MATCHES) truncated = true
    candidates.sort(
      (left, right) => left.score - right.score || left.displayPath.localeCompare(right.displayPath)
    )
    const entries = candidates.slice(0, limit).map((entry) => ({
      alias: entry.alias,
      displayName: entry.displayName,
      folderId: entry.folderId,
      kind: entry.kind,
      path: entry.path,
      displayPath: entry.displayPath
    }))
    return { entries, truncated: truncated || candidates.length > limit }
  }

  async listDirectory(input: WorkspaceListDirectoryInput): Promise<WorkspaceDirectoryListing> {
    const projectId = requireProjectId(input?.projectId)
    const directoryPath = normalizeWorkspacePath(input?.directoryPath ?? '', true)
    const directory = await this.resolveEntry(
      projectId,
      directoryPath,
      normalizeFolderId(input?.folderId)
    )
    if (directory.kind === 'symlink' || !directory.realPath) {
      throw new Error('Symbolic-link directories are not available in the file browser')
    }

    const directoryStat = await lstat(directory.realPath)
    if (!directoryStat.isDirectory()) {
      throw new Error('The requested workspace path is not a directory')
    }

    const includeHidden = input?.includeHidden !== false
    const rawEntries = await readdir(directory.realPath, { withFileTypes: true })
    const entries = rawEntries
      .filter((entry) => entry.name !== '.git')
      .filter((entry) => includeHidden || !entry.name.startsWith('.'))
      .map((entry): WorkspaceDirectoryEntry => {
        const kind: WorkspaceDirectoryEntryKind = entry.isDirectory()
          ? 'directory'
          : entry.isSymbolicLink()
            ? 'symlink'
            : 'file'
        const path = joinWorkspacePath(directoryPath, entry.name)
        return {
          kind,
          name: entry.name,
          path: kind === 'directory' ? `${path}/` : path
        }
      })
      .sort(compareDirectoryEntries)

    return {
      directoryPath,
      entries: entries.slice(0, DIRECTORY_ENTRY_LIMIT),
      truncated: entries.length > DIRECTORY_ENTRY_LIMIT
    }
  }

  async readPreview(input: WorkspaceFileRequest): Promise<WorkspaceFilePreviewResult> {
    const request = normalizeFileRequest(input)
    const entry = await this.resolveFileRequest(request)

    if (entry.kind === 'symlink' || !entry.realPath) {
      return { metadata: metadataFromEntry(entry, request.path, 'unsupported', null) }
    }

    const fileStat = await lstat(entry.realPath)
    const currentEntry: ResolvedWorkspaceEntry = {
      ...entry,
      modifiedAtMs: fileStat.mtimeMs,
      sizeBytes: fileStat.size
    }
    if (!fileStat.isFile()) {
      return { metadata: metadataFromEntry(currentEntry, request.path, 'unsupported', null) }
    }

    const extension = extname(request.path).toLowerCase()
    if (extension === '.pdf') {
      const mimeType = 'application/pdf' as const
      if (currentEntry.sizeBytes > PDF_PREVIEW_LIMIT_BYTES) {
        return {
          metadata: metadataFromEntry(currentEntry, request.path, 'too-large', mimeType)
        }
      }

      const data = await readBoundedFile(
        entry.realPath,
        PDF_PREVIEW_LIMIT_BYTES,
        currentEntry.sizeBytes
      )
      if (!hasPdfHeader(data)) {
        return {
          metadata: metadataFromEntry(currentEntry, request.path, 'unsupported', mimeType)
        }
      }

      return {
        metadata: metadataFromEntry(currentEntry, request.path, 'pdf', mimeType),
        pdf: {
          data,
          mimeType,
          modifiedAtMs: currentEntry.modifiedAtMs,
          path: request.path,
          sizeBytes: data.byteLength
        }
      }
    }

    const mimeType = IMAGE_MIME_BY_EXTENSION[extension] ?? null
    if (mimeType) {
      if (currentEntry.sizeBytes > IMAGE_PREVIEW_LIMIT_BYTES) {
        return {
          metadata: metadataFromEntry(currentEntry, request.path, 'too-large', mimeType)
        }
      }
      const data = await readBoundedFile(
        entry.realPath,
        IMAGE_PREVIEW_LIMIT_BYTES,
        currentEntry.sizeBytes
      )
      return {
        image: {
          data: data.toString('base64'),
          mimeType,
          modifiedAtMs: currentEntry.modifiedAtMs,
          path: request.path,
          sizeBytes: data.byteLength
        },
        metadata: metadataFromEntry(currentEntry, request.path, 'image', mimeType)
      }
    }

    if (currentEntry.sizeBytes > TEXT_PREVIEW_LIMIT_BYTES) {
      return { metadata: metadataFromEntry(currentEntry, request.path, 'too-large', null) }
    }

    const data = await readBoundedFile(
      entry.realPath,
      TEXT_PREVIEW_LIMIT_BYTES,
      currentEntry.sizeBytes
    )
    const decodedText = decodeWorkspaceText(data)
    if (!decodedText) {
      return { metadata: metadataFromEntry(currentEntry, request.path, 'binary', null) }
    }

    return {
      metadata: metadataFromEntry(currentEntry, request.path, 'text', decodedText.mimeType),
      text: {
        content: decodedText.content,
        modifiedAtMs: currentEntry.modifiedAtMs,
        path: request.path,
        sizeBytes: data.byteLength
      }
    }
  }

  async resolvePathForReveal(input: WorkspaceFileRequest): Promise<string> {
    const request = normalizeFileRequest(input)
    const entry = await this.resolveFileRequest(request)
    if (!entry.realPath) {
      throw new Error('Symbolic links cannot be revealed from the file browser')
    }
    return entry.realPath
  }

  private async resolveFileRequest(request: WorkspaceFileRequest): Promise<ResolvedWorkspaceEntry> {
    if (request.assistantMessageId === undefined) {
      return this.resolveEntry(request.projectId, request.path, request.folderId)
    }
    if (!this.runFiles) throw new Error('Historical workspace is not available')
    const identity = {
      assistantMessageId: request.assistantMessageId,
      projectId: request.projectId
    }
    const workspace = await this.runFiles.loadRunWorkspace(identity)
    if (!workspace || workspace.projectId !== request.projectId) {
      throw new Error('Historical workspace is not available')
    }
    const folders = workspace.folders.filter((folder) =>
      request.folderId === undefined ? folder.role === 'primary' : folder.id === request.folderId
    )
    if (folders.length !== 1) throw new Error('Historical workspace folder is not available')
    const resolvedPath = await this.runFiles.resolveRunWorkspacePath({
      ...identity,
      filePath: `@workspace/${folders[0].alias}/${request.path}`
    })
    const info = await lstat(resolvedPath)
    return {
      kind: info.isSymbolicLink() ? 'symlink' : info.isDirectory() ? 'directory' : 'file',
      modifiedAtMs: info.mtimeMs,
      path: request.path,
      realPath: info.isSymbolicLink() ? null : resolvedPath,
      sizeBytes: info.size
    }
  }

  private async resolveEntry(
    projectId: string,
    relativePath: string,
    folderId: string | undefined
  ): Promise<ResolvedWorkspaceEntry> {
    const project = await this.resolveProject(projectId)
    if (!project || project.id !== projectId) throw new Error('Project is not available')
    const folders = project.folders.filter((folder) =>
      folderId === undefined ? folder.role === 'primary' : folder.id === folderId
    )
    if (folders.length !== 1) throw new Error('Workspace folder is not available')
    const configuredRoot = folders[0].path
    if (!configuredRoot?.trim()) throw new Error('Project path is not available')

    const root = await realpath(resolve(configuredRoot))
    const candidate = resolve(root, relativePath)
    assertPathWithinRoot(root, candidate)

    const candidateStat = await lstat(candidate)
    if (candidateStat.isSymbolicLink()) {
      return {
        kind: 'symlink',
        modifiedAtMs: candidateStat.mtimeMs,
        path: relativePath,
        realPath: null,
        sizeBytes: candidateStat.size
      }
    }

    const resolvedCandidate = await realpath(candidate)
    assertPathWithinRoot(root, resolvedCandidate)
    return {
      kind: candidateStat.isDirectory() ? 'directory' : 'file',
      modifiedAtMs: candidateStat.mtimeMs,
      path: relativePath,
      realPath: resolvedCandidate,
      sizeBytes: candidateStat.size
    }
  }
}

export function normalizeWorkspacePath(value: unknown, allowRoot = false): string {
  if (typeof value !== 'string') throw new Error('Workspace path must be a string')
  if (!value) {
    if (allowRoot) return ''
    throw new Error('Workspace path is required')
  }
  if (value.includes('\0') || isAbsolute(value) || win32.isAbsolute(value)) {
    throw new Error('Workspace path must be relative')
  }

  const segments = value.split(/[\\/]+/)
  if (segments.some((segment) => !segment || segment === '.' || segment === '..')) {
    throw new Error('Workspace path contains an invalid segment')
  }
  return segments.join('/')
}

function normalizeFileRequest(input: WorkspaceFileRequest): WorkspaceFileRequest {
  return {
    assistantMessageId: normalizeOptionalIdentity(
      input?.assistantMessageId,
      'Assistant message id'
    ),
    folderId: normalizeFolderId(input?.folderId),
    path: normalizeWorkspacePath(input?.path),
    projectId: requireProjectId(input?.projectId)
  }
}

function normalizeFolderId(value: unknown): string | undefined {
  return normalizeOptionalIdentity(value, 'Workspace folder id')
}

function normalizeOptionalIdentity(value: unknown, label: string): string | undefined {
  if (value === undefined) return undefined
  if (typeof value !== 'string' || !value.trim()) throw new Error(`${label} is invalid`)
  return value.trim()
}

function requireProjectId(value: unknown): string {
  if (typeof value !== 'string' || !value.trim()) throw new Error('Project id is required')
  return value.trim()
}

function normalizeMentionQuery(value: unknown): string {
  if (typeof value !== 'string') throw new Error('Workspace search query must be a string')
  const query = value.trim()
  if (query.length > MENTION_SEARCH_MAX_QUERY_LENGTH) {
    throw new Error('Workspace search query is too long')
  }
  return query.toLocaleLowerCase()
}

function clampMentionLimit(value: unknown): number {
  if (value === undefined) return MENTION_SEARCH_DEFAULT_LIMIT
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    throw new Error('Workspace search limit is invalid')
  }
  return Math.min(MENTION_SEARCH_MAX_LIMIT, Math.max(1, Math.floor(value)))
}

/** Lower scores are better. Matching is subsequence based, with a small path-depth preference. */
function mentionMatchScore(query: string, value: string): number | null {
  const normalized = value.toLocaleLowerCase()
  let queryIndex = 0
  let firstMatch = -1
  let lastMatch = -1
  for (let index = 0; index < normalized.length && queryIndex < query.length; index += 1) {
    if (normalized[index] !== query[queryIndex]) continue
    if (firstMatch === -1) firstMatch = index
    lastMatch = index
    queryIndex += 1
  }
  if (queryIndex !== query.length) return null
  const separators = (normalized.match(/[\\/]/g) ?? []).length
  const basenameBonus = normalized.endsWith(query) ? -20 : 0
  return (
    firstMatch + (lastMatch - firstMatch - query.length + 1) * 2 + separators * 3 + basenameBonus
  )
}

function joinWorkspacePath(parent: string, name: string): string {
  return parent ? `${parent}/${name}` : name
}

function assertPathWithinRoot(root: string, candidate: string): void {
  const relativePath = relative(root, candidate)
  if (
    relativePath === '..' ||
    relativePath.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) ||
    isAbsolute(relativePath)
  ) {
    throw new Error('Workspace path resolves outside the project')
  }
}

function compareDirectoryEntries(
  left: WorkspaceDirectoryEntry,
  right: WorkspaceDirectoryEntry
): number {
  const rank = (kind: WorkspaceDirectoryEntryKind): number =>
    kind === 'directory' ? 0 : kind === 'file' ? 1 : 2
  return (
    rank(left.kind) - rank(right.kind) ||
    left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: 'base' })
  )
}

function metadataFromEntry(
  entry: ResolvedWorkspaceEntry,
  path: string,
  previewKind: WorkspaceFileMetadata['previewKind'],
  mimeType: string | null
): WorkspaceFileMetadata {
  return {
    kind: entry.kind,
    mimeType,
    modifiedAtMs: entry.modifiedAtMs,
    path,
    previewKind,
    sizeBytes: entry.sizeBytes
  }
}

interface DecodedWorkspaceText {
  content: string
  mimeType: string
}

function decodeWorkspaceText(data: Buffer): DecodedWorkspaceText | null {
  let content: string
  let mimeType: string

  try {
    if (data[0] === 0xff && data[1] === 0xfe) {
      content = new TextDecoder('utf-16le', { fatal: true }).decode(data)
      mimeType = 'text/plain; charset=utf-16le'
    } else if (data[0] === 0xfe && data[1] === 0xff) {
      content = new TextDecoder('utf-16be', { fatal: true }).decode(data)
      mimeType = 'text/plain; charset=utf-16be'
    } else {
      if (!isUtf8(data)) return null
      content = data.toString('utf8')
      mimeType = 'text/plain; charset=utf-8'
    }
  } catch {
    return null
  }

  if (content.startsWith('\uFEFF')) content = content.slice(1)
  return isLikelyTextContent(content) ? { content, mimeType } : null
}

function isLikelyTextContent(content: string): boolean {
  if (content.includes('\0')) return false
  if (content.length === 0) return true

  let controlCharacters = 0
  let totalCharacters = 0
  for (const character of content) {
    totalCharacters += 1
    const codePoint = character.codePointAt(0) ?? 0
    const isAllowedWhitespace = codePoint === 9 || codePoint === 10 || codePoint === 13
    if (!isAllowedWhitespace && (codePoint < 32 || (codePoint >= 127 && codePoint <= 159))) {
      controlCharacters += 1
    }
  }
  return controlCharacters / totalCharacters < 0.08
}

function hasPdfHeader(data: Buffer): boolean {
  return data.subarray(0, PDF_HEADER_SEARCH_BYTES).indexOf(PDF_HEADER) >= 0
}

async function readBoundedFile(
  filePath: string,
  limit: number,
  expectedSize: number
): Promise<Buffer> {
  const file = await open(filePath, 'r')
  try {
    let buffer = Buffer.allocUnsafe(Math.min(limit + 1, Math.max(1, expectedSize + 1)))
    let offset = 0
    while (true) {
      if (offset === buffer.byteLength) {
        if (offset > limit) throw new Error('Workspace file exceeds the preview size limit')
        const nextCapacity = Math.min(
          limit + 1,
          Math.max(buffer.byteLength * 2, offset + 64 * 1024)
        )
        if (nextCapacity <= buffer.byteLength) {
          throw new Error('Workspace file exceeds the preview size limit')
        }
        const expanded = Buffer.allocUnsafe(nextCapacity)
        buffer.copy(expanded, 0, 0, offset)
        buffer = expanded
      }
      const { bytesRead } = await file.read(buffer, offset, buffer.byteLength - offset, offset)
      if (bytesRead === 0) break
      offset += bytesRead
    }
    if (offset > limit) throw new Error('Workspace file exceeds the preview size limit')
    return buffer.subarray(0, offset)
  } finally {
    await file.close()
  }
}
