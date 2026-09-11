import { randomUUID } from 'crypto'
import { realpath, stat } from 'fs/promises'
import { basename, resolve, sep } from 'path'
import {
  MAX_PROJECT_FOLDERS,
  type StorageProjectCreateInput,
  type StorageProjectFolderInput,
  type StorageProjectFolderRecord,
  type StorageProjectRecord,
  type StorageProjectUpdateInput,
  type StorageProjectValidationCode,
  type StorageProjectValidationErrorData
} from '@mycopilot/protocol'

const MAX_PROJECT_FOLDER_ALIAS_LENGTH = 64
const FALLBACK_PROJECT_FOLDER_ALIAS = 'workspace'

/**
 * Structured rejection of a project create/update request. Only `data` crosses into the
 * renderer; the message stays generic so filesystem details never leak through error text.
 */
export class ProjectValidationError extends Error {
  readonly data: StorageProjectValidationErrorData

  constructor(code: StorageProjectValidationCode, path?: string) {
    super(`Project validation failed: ${code}`)
    this.name = 'ProjectValidationError'
    this.data = { kind: 'project_validation', code, ...(path === undefined ? {} : { path }) }
  }
}

export interface ProjectRecordFactoryOptions {
  now?: () => number
  folderId?: () => string
}

interface ResolvedFolderInput {
  input: StorageProjectFolderInput
  path: string
  realPath: string
}

/**
 * Derives the stable, project-unique alias for a folder from its final path component.
 * The alias is assigned once when a folder joins a project and never recomputed afterwards.
 */
export function projectFolderAliasFromPath(folderPath: string): string {
  const base = basename(folderPath.replace(/[\\/]+$/, ''))
  const normalized = base
    .normalize('NFC')
    .trim()
    .replace(/\s+/g, '-')
    .replace(/[^\p{L}\p{N}._-]+/gu, '')
    .replace(/-{2,}/g, '-')
    .replace(/^[-.]+|[-.]+$/g, '')
    .slice(0, MAX_PROJECT_FOLDER_ALIAS_LENGTH)
    .replace(/^[-.]+|[-.]+$/g, '')
  return normalized.length > 0 ? normalized : FALLBACK_PROJECT_FOLDER_ALIAS
}

function uniqueProjectFolderAlias(candidate: string, taken: Set<string>): string {
  if (!taken.has(candidate)) return candidate
  for (let suffix = 2; ; suffix += 1) {
    const alias = `${candidate}-${suffix}`
    if (!taken.has(alias)) return alias
  }
}

function projectIdFromName(name: string, now: number): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return `project-${slug || 'workspace'}-${now}`
}

function isNestedPath(parent: string, child: string): boolean {
  const prefix = parent.endsWith(sep) ? parent : `${parent}${sep}`
  return child.startsWith(prefix)
}

async function resolveFolderInputs(
  folders: StorageProjectFolderInput[]
): Promise<ResolvedFolderInput[]> {
  if (folders.length === 0) throw new ProjectValidationError('folders_required')
  if (folders.length > MAX_PROJECT_FOLDERS) throw new ProjectValidationError('too_many_folders')
  if (folders.filter((folder) => folder.role === 'primary').length !== 1) {
    throw new ProjectValidationError('primary_required')
  }

  const resolved: ResolvedFolderInput[] = []
  for (const input of folders) {
    const trimmed = input.path.trim()
    if (!trimmed) throw new ProjectValidationError('folder_missing', input.path)
    const path = resolve(trimmed)
    let realPath: string
    try {
      const info = await stat(path)
      if (!info.isDirectory()) throw new ProjectValidationError('folder_missing', path)
      realPath = await realpath(path)
    } catch (error) {
      if (error instanceof ProjectValidationError) throw error
      throw new ProjectValidationError('folder_missing', path)
    }
    resolved.push({ input, path, realPath })
  }

  for (let index = 0; index < resolved.length; index += 1) {
    for (let other = 0; other < index; other += 1) {
      const current = resolved[index]
      const previous = resolved[other]
      if (current.realPath === previous.realPath) {
        throw new ProjectValidationError('folder_duplicate', current.path)
      }
      if (isNestedPath(previous.realPath, current.realPath)) {
        throw new ProjectValidationError('folder_nested', current.path)
      }
      if (isNestedPath(current.realPath, previous.realPath)) {
        throw new ProjectValidationError('folder_nested', current.path)
      }
    }
  }
  return resolved
}

function buildFolderRecords(
  resolved: ResolvedFolderInput[],
  existing: StorageProjectFolderRecord[],
  now: number,
  folderId: () => string
): StorageProjectFolderRecord[] {
  const existingById = new Map(existing.map((folder) => [folder.id, folder]))
  const kept: (StorageProjectFolderRecord | null)[] = resolved.map(({ input, path }) => {
    const previous = input.id ? existingById.get(input.id) : undefined
    // A folder keeps its identity (id, alias, created_at) only while it still points at the
    // same path; re-pointing a slot is treated as removing one folder and adding another.
    if (!previous || previous.path !== path) return null
    return { ...previous, path, role: input.role }
  })
  const takenAliases = new Set(kept.flatMap((folder) => (folder === null ? [] : [folder.alias])))
  return resolved.map(({ input, path }, index) => {
    const previous = kept[index]
    if (previous) return { ...previous, sortOrder: index }
    const alias = uniqueProjectFolderAlias(projectFolderAliasFromPath(path), takenAliases)
    takenAliases.add(alias)
    return { id: folderId(), path, alias, role: input.role, sortOrder: index, createdAt: now }
  })
}

function requireProjectName(name: string): string {
  const trimmed = name.trim()
  if (!trimmed) throw new ProjectValidationError('name_required')
  return trimmed
}

/** Validates a create request against the filesystem and produces the record Core will store. */
export async function buildProjectRecordFromCreateInput(
  input: StorageProjectCreateInput,
  options: ProjectRecordFactoryOptions = {}
): Promise<StorageProjectRecord> {
  const now = options.now?.() ?? Date.now()
  const folderId = options.folderId ?? (() => `folder-${randomUUID()}`)
  const name = requireProjectName(input.name)
  const resolved = await resolveFolderInputs(input.folders)
  return {
    id: projectIdFromName(name, now),
    name,
    folders: buildFolderRecords(resolved, [], now, folderId),
    createdAt: now,
    pinnedAt: null
  }
}

/**
 * Validates an update request and merges it onto the stored record: identity, creation time
 * and pin state are preserved, kept folders retain their ids and aliases.
 */
export async function buildProjectRecordFromUpdateInput(
  input: StorageProjectUpdateInput,
  existing: StorageProjectRecord | undefined,
  options: ProjectRecordFactoryOptions = {}
): Promise<StorageProjectRecord> {
  if (!existing) throw new ProjectValidationError('project_missing')
  const now = options.now?.() ?? Date.now()
  const folderId = options.folderId ?? (() => `folder-${randomUUID()}`)
  const name = requireProjectName(input.name)
  const resolved = await resolveFolderInputs(input.folders)
  return {
    ...existing,
    name,
    folders: buildFolderRecords(resolved, existing.folders, now, folderId)
  }
}

/** Filesystem path of the primary folder, which stays the project's working directory. */
export function primaryProjectFolderPath(project: StorageProjectRecord | undefined): string | null {
  return project?.folders.find((folder) => folder.role === 'primary')?.path ?? null
}
