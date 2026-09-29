import { realpath, stat } from 'node:fs/promises'
import { isAbsolute } from 'node:path'
import type { AgentFolderReference } from '@mycopilot/protocol'

/** Folder chips open the selected directory itself, never execute a file at that path. */
export async function resolveAttachmentFolderForOpen(
  folder: AgentFolderReference
): Promise<string> {
  if (
    !folder ||
    folder.status === 'unavailable' ||
    typeof folder.rootPath !== 'string' ||
    !isAbsolute(folder.rootPath) ||
    folder.rootPath.includes('\0')
  ) {
    throw new Error('The attached folder is no longer available.')
  }
  const path = await realpath(folder.rootPath)
  const info = await stat(path)
  if (!info.isDirectory()) throw new Error('The attached path is not a folder.')
  if (
    folder.rootIdentity?.kind === 'unix' &&
    (folder.rootIdentity.device !== info.dev || folder.rootIdentity.inode !== info.ino)
  ) {
    throw new Error('The attached folder has been replaced.')
  }
  return path
}
