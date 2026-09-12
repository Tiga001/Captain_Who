import { realpath, stat } from 'node:fs/promises'
import { homedir } from 'node:os'
import { isAbsolute, join, relative, resolve, win32 } from 'node:path'
import type { StorageProjectRecord } from '@mycopilot/protocol'

export interface ProjectFileReference {
  projectId?: string | null
  filePath: string
  /** Historical tool results must resolve against this assistant Turn's frozen workspace. */
  assistantMessageId?: string
}

interface ProjectFilePathSource {
  loadProjects(): Promise<StorageProjectRecord[]>
  resolveRunWorkspacePath(input: {
    assistantMessageId: string
    projectId?: string | null
    filePath: string
  }): Promise<string>
}

const unavailable = () => new Error('The referenced workspace is no longer available.')

function assertWithinRoot(root: string, path: string): void {
  const child = relative(root, path)
  if (child === '..' || child.startsWith('../') || child.startsWith('..\\') || isAbsolute(child)) {
    throw unavailable()
  }
}

/** Current project actions use current folders; historical actions never fall back to them. */
export async function resolveProjectFileReference(
  source: ProjectFilePathSource,
  input: ProjectFileReference
): Promise<string> {
  const raw = input.filePath.trim()
  if (!raw || /[\0\r\n]/.test(raw)) throw unavailable()
  if (raw.startsWith('@workspace') && raw.includes('\\')) throw unavailable()
  if (input.assistantMessageId !== undefined) {
    if (!input.assistantMessageId.trim()) throw unavailable()
    // Rust owns frozen root and cross-platform filesystem identity validation.
    return source.resolveRunWorkspacePath({
      assistantMessageId: input.assistantMessageId,
      projectId: input.projectId,
      filePath: raw
    })
  }
  if (isAbsolute(raw)) return raw
  if (win32.isAbsolute(raw)) throw unavailable()

  const normalized = raw.replaceAll('\\', '/')
  const segments = normalized.split('/')
  const literalWorkspaceDirectory = segments[0] === '.'
  let alias: string | undefined
  if (!literalWorkspaceDirectory && segments[0] === '@workspace') {
    segments.shift()
    alias = segments.shift()
    if (!alias) throw unavailable()
  } else if (!literalWorkspaceDirectory && segments[0].startsWith('@')) {
    const systemRoots: Record<string, string> = {
      '@desktop': join(homedir(), 'Desktop'),
      '@documents': join(homedir(), 'Documents'),
      '@downloads': join(homedir(), 'Downloads'),
      '@home': homedir()
    }
    const systemRoot = systemRoots[segments.shift()!.toLowerCase()]
    if (!systemRoot || segments.some((segment) => segment === '..')) throw unavailable()
    return resolve(systemRoot, ...segments)
  }
  if (segments.some((segment) => segment === '..')) throw unavailable()
  const cleanSegments = segments.filter((segment) => segment && segment !== '.')

  if (!input.projectId) throw unavailable()
  const project = (await source.loadProjects()).find((project) => project.id === input.projectId)
  const folders = project?.folders ?? []
  const matches = folders.filter((folder) =>
    alias ? folder.alias === alias : folder.role === 'primary'
  )
  if (matches.length !== 1) throw unavailable()
  const folder = matches[0]
  const root = await realpath(resolve(folder.path))
  const info = await stat(root, { bigint: true })
  if (!info.isDirectory()) throw unavailable()
  const candidate = resolve(root, ...cleanSegments)
  assertWithinRoot(root, candidate)
  const resolvedCandidate = await realpath(candidate)
  assertWithinRoot(root, resolvedCandidate)
  return resolvedCandidate
}
