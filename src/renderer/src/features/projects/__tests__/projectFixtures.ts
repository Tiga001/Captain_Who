import type { AppProject } from '../../../config/projectConfig'

interface SingleFolderProjectInput {
  createdAt?: number
  id: string
  name: string
  path?: string
  pinnedAt?: number | null
}

/** Builds the common one-folder project shape for renderer tests. */
export function singleFolderProject({
  createdAt = 1,
  id,
  name,
  path = `/workspace/${id}`,
  pinnedAt
}: SingleFolderProjectInput): AppProject {
  return {
    id,
    name,
    folders: [
      {
        id: `${id}-primary`,
        path,
        alias: path.split('/').filter(Boolean).at(-1) ?? 'workspace',
        role: 'primary',
        sortOrder: 0,
        createdAt
      }
    ],
    createdAt,
    ...(pinnedAt === undefined ? {} : { pinnedAt })
  }
}
