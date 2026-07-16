import { isUtf8 } from 'node:buffer'
import { lstat, open, readdir, realpath } from 'node:fs/promises'
import { extname, isAbsolute, relative, resolve, win32 } from 'node:path'
import type {
  WorkspaceDirectoryEntry,
  WorkspaceDirectoryEntryKind,
  WorkspaceDirectoryListing,
  WorkspaceFileMetadata,
  WorkspaceFileRequest,
  WorkspaceImageFileContent,
  WorkspaceListDirectoryInput,
  WorkspaceTextFileContent
} from '@mycopilot/protocol'

const DIRECTORY_ENTRY_LIMIT = 20_000
const FILE_SAMPLE_BYTES = 8 * 1024
const IMAGE_PREVIEW_LIMIT_BYTES = 12 * 1024 * 1024
const TEXT_PREVIEW_LIMIT_BYTES = 1024 * 1024

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

  async readFileMetadata(input: WorkspaceFileRequest): Promise<WorkspaceFileMetadata> {
    const request = normalizeFileRequest(input)
    const entry = await this.resolveEntry(request.projectId, request.path)

    if (entry.kind === 'symlink' || !entry.realPath) {
      return metadataFromEntry(entry, request.path, 'unsupported', null)
    }

    const fileStat = await lstat(entry.realPath)
    if (!fileStat.isFile()) {
      return metadataFromEntry(entry, request.path, 'unsupported', null)
    }

    const mimeType = IMAGE_MIME_BY_EXTENSION[extname(request.path).toLowerCase()] ?? null
    if (mimeType) {
      return metadataFromEntry(
        entry,
        request.path,
        entry.sizeBytes > IMAGE_PREVIEW_LIMIT_BYTES ? 'too-large' : 'image',
        mimeType
      )
    }

    const sample = await readPrefix(entry.realPath, FILE_SAMPLE_BYTES)
    const isText = isLikelyUtf8Text(sample)
    return metadataFromEntry(
      entry,
      request.path,
      isText ? (entry.sizeBytes > TEXT_PREVIEW_LIMIT_BYTES ? 'too-large' : 'text') : 'binary',
      isText ? 'text/plain; charset=utf-8' : null
    )
  }

  async readTextFile(input: WorkspaceFileRequest): Promise<WorkspaceTextFileContent> {
    const request = normalizeFileRequest(input)
    const metadata = await this.readFileMetadata(request)
    if (metadata.previewKind !== 'text') {
      throw new Error('The requested workspace file is not previewable text')
    }

    const entry = await this.resolveReadableFile(request)
    const data = await readBoundedFile(entry.realPath, TEXT_PREVIEW_LIMIT_BYTES)
    if (!isLikelyUtf8Text(data)) {
      throw new Error('The workspace file is not valid UTF-8 text')
    }

    return {
      content: data.toString('utf8'),
      modifiedAtMs: entry.modifiedAtMs,
      path: request.path,
      sizeBytes: data.byteLength
    }
  }

  async readImageFile(input: WorkspaceFileRequest): Promise<WorkspaceImageFileContent> {
    const request = normalizeFileRequest(input)
    const metadata = await this.readFileMetadata(request)
    if (metadata.previewKind !== 'image' || !metadata.mimeType) {
      throw new Error('The requested workspace file is not a previewable image')
    }

    const entry = await this.resolveReadableFile(request)
    const data = await readBoundedFile(entry.realPath, IMAGE_PREVIEW_LIMIT_BYTES)
    return {
      data: data.toString('base64'),
      mimeType: metadata.mimeType,
      modifiedAtMs: entry.modifiedAtMs,
      path: request.path,
      sizeBytes: data.byteLength
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

  private async resolveReadableFile(request: WorkspaceFileRequest): Promise<{
    modifiedAtMs: number
    realPath: string
  }> {
    const entry = await this.resolveEntry(request.projectId, request.path)
    if (entry.kind === 'symlink' || !entry.realPath) {
      throw new Error('Symbolic links cannot be read from the file browser')
    }
    const fileStat = await lstat(entry.realPath)
    if (!fileStat.isFile()) throw new Error('The requested workspace path is not a file')
    return { modifiedAtMs: entry.modifiedAtMs, realPath: entry.realPath }
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

function isLikelyUtf8Text(data: Uint8Array): boolean {
  if (!isUtf8(data)) return false
  if (data.includes(0)) return false
  if (data.byteLength === 0) return true

  let controlBytes = 0
  for (const byte of data) {
    if (byte < 32 && byte !== 9 && byte !== 10 && byte !== 13) controlBytes += 1
  }
  return controlBytes / data.byteLength < 0.08
}

async function readPrefix(filePath: string, limit: number): Promise<Buffer> {
  const file = await open(filePath, 'r')
  try {
    const buffer = Buffer.allocUnsafe(limit)
    const { bytesRead } = await file.read(buffer, 0, limit, 0)
    return buffer.subarray(0, bytesRead)
  } finally {
    await file.close()
  }
}

async function readBoundedFile(filePath: string, limit: number): Promise<Buffer> {
  const file = await open(filePath, 'r')
  try {
    const buffer = Buffer.allocUnsafe(limit + 1)
    let offset = 0
    while (offset < buffer.byteLength) {
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
