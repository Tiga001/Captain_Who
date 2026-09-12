export type AppProjectFolderRole = 'primary' | 'auxiliary'

/** One filesystem root of a project. The alias is assigned once by the host and never changes. */
export interface AppProjectFolder {
  id: string
  path: string
  alias: string
  role: AppProjectFolderRole
  sortOrder: number
  createdAt: number
}

export interface AppProject {
  id: string
  name: string
  /** Display order; exactly one folder is `primary` whenever the list is non-empty. */
  folders: AppProjectFolder[]
  createdAt: number
  pinnedAt?: number | null
}

export function primaryProjectFolder(
  project: Pick<AppProject, 'folders'> | null | undefined
): AppProjectFolder | undefined {
  return project?.folders.find((folder) => folder.role === 'primary')
}

/** The primary folder stays the project's working directory (Git, terminal, file reveal). */
export function primaryProjectPath(
  project: Pick<AppProject, 'folders'> | null | undefined
): string | undefined {
  const path = primaryProjectFolder(project)?.path.trim()
  return path ? path : undefined
}

/** Folders in the stored display order. */
export function sortedProjectFolders(
  project: Pick<AppProject, 'folders'> | null | undefined
): AppProjectFolder[] {
  if (!project) return []
  return [...project.folders].sort(
    (left, right) => left.sortOrder - right.sortOrder || left.createdAt - right.createdAt
  )
}

/** Last path component, shown as the folder's label in project UI. */
export function projectFolderDisplayName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, '')
  const segments = trimmed.split(/[\\/]/)
  return segments[segments.length - 1] || trimmed || path
}

/** Shortens a user-home path to `~/...` for compact project UI. */
export function formatProjectFolderPath(path: string): string {
  const trimmed = path.trim()
  const unixHome = trimmed.match(/^\/(?:Users|home)\/[^/]+/)
  if (unixHome) {
    const rest = trimmed.slice(unixHome[0].length)
    return rest ? `~${rest}` : '~'
  }
  const windowsHome = trimmed.match(/^[A-Za-z]:\\Users\\[^\\]+/)
  if (windowsHome) {
    const rest = trimmed.slice(windowsHome[0].length).replaceAll('\\', '/')
    return rest ? `~${rest}` : '~'
  }
  return trimmed
}

/** Invalidates root-bound previews when membership or the primary-folder role changes. */
export function projectWorkspaceRevision(
  project: Pick<AppProject, 'folders'> | null | undefined
): string {
  return JSON.stringify(
    project?.folders.map(({ id, alias, role, path }) => ({ id, alias, role, path })) ?? []
  )
}
