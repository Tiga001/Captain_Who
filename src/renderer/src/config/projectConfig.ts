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

/** Last path component, shown as the folder's label in project UI. */
export function projectFolderDisplayName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, '')
  const segments = trimmed.split(/[\\/]/)
  return segments[segments.length - 1] || trimmed || path
}
