import { isUtf8 } from 'node:buffer'
import { lstat, open, readdir, realpath } from 'node:fs/promises'
import { extname, isAbsolute, relative, resolve, win32 } from 'node:path'
import type {
  WorkspaceDirectoryEntry,
  WorkspaceDirectoryEntryKind,
  WorkspaceDirectoryListing,
  WorkspaceFileMetadata,
  WorkspaceFilePreviewResult,
  WorkspaceFileRequest,
  WorkspaceListDirectoryInput
} from '@mycopilot/protocol'

const DIRECTORY_ENTRY_LIMIT = 20_000
const IMAGE_PREVIEW_LIMIT_BYTES = 12 * 1024 * 1024
export const PDF_PREVIEW_LIMIT_BYTES = 32 * 1024 * 1024
const TEXT_PREVIEW_LIMIT_BYTES = 1024 * 1024
const PDF_HEADER = Buffer.from('%PDF-')
const PDF_HEADER_SEARCH_BYTES = 1024

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

export type ProjectPathResolver = (projectId: string) => Promise<string | null>

interface ResolvedWorkspaceEntry {
  kind: WorkspaceDirectoryEntryKind
  modifiedAtMs: number
  path: string
  realPath: string | null
  sizeBytes: number
}

export class WorkspaceFilesService {
  constructor(private readonly resolveProjectPath: ProjectPathResolver) {}

  async listDirectory(input: WorkspaceListDirectoryInput): Promise<WorkspaceDirectoryListing> {
    const projectId = requireProjectId(input?.projectId)
    const directoryPath = normalizeWorkspacePath(input?.directoryPath ?? '', true)
    const directory = await this.resolveEntry(projectId, directoryPath)
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
    const entry = await this.resolveEntry(request.projectId, request.path)

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
    const entry = await this.resolveEntry(request.projectId, request.path)
    if (!entry.realPath) {
      throw new Error('Symbolic links cannot be revealed from the file browser')
    }
    return entry.realPath
  }

  private async resolveEntry(
    projectId: string,
    relativePath: string
  ): Promise<ResolvedWorkspaceEntry> {
    const configuredRoot = await this.resolveProjectPath(projectId)
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
    path: normalizeWorkspacePath(input?.path),
    projectId: requireProjectId(input?.projectId)
  }
}

function requireProjectId(value: unknown): string {
  if (typeof value !== 'string' || !value.trim()) throw new Error('Project id is required')
  return value.trim()
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
