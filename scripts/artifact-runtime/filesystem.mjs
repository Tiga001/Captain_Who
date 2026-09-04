/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { createHash } from 'node:crypto'
import { constants as fsConstants } from 'node:fs'
import { createReadStream } from 'node:fs'
import { lstat, open } from 'node:fs/promises'

export async function hashFile(path) {
  const digest = createHash('sha256')
  for await (const chunk of createReadStream(path)) {
    digest.update(chunk)
  }
  return digest.digest('hex')
}

export async function verifyPinnedLocalFile(path, expectedSha256, label) {
  let metadata
  try {
    metadata = await lstat(path)
  } catch (error) {
    throw new Error(`${label} is unavailable: ${error.message}`, { cause: error })
  }
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`${label} must be a regular non-symlink file`)
  }
  const actual = await hashFile(path)
  if (actual !== expectedSha256) {
    throw new Error(`${label} SHA-256 does not match the pinned Artifact Runtime manifest`)
  }
}

export async function fileMatches(path, descriptor) {
  try {
    const metadata = await lstat(path)
    return (
      metadata.isFile() &&
      !metadata.isSymbolicLink() &&
      metadata.size === descriptor.size &&
      (await hashFile(path)) === descriptor.sha256
    )
  } catch (error) {
    if (error?.code === 'ENOENT') return false
    throw error
  }
}

export async function readRegularFileNoFollow(path, label) {
  const metadata = await lstat(path)
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`${label} must be a regular non-symlink file: ${path}`)
  }
  const flags = fsConstants.O_RDONLY | (fsConstants.O_NOFOLLOW ?? 0)
  let handle
  try {
    handle = await open(path, flags)
  } catch (error) {
    throw new Error(`${label} cannot be opened without following links: ${path}`, { cause: error })
  }
  try {
    const opened = await handle.stat()
    if (!opened.isFile()) throw new Error(`${label} is not a regular file: ${path}`)
    if (
      opened.size !== metadata.size ||
      (metadata.dev !== undefined && opened.dev !== metadata.dev) ||
      (metadata.ino !== undefined && opened.ino !== metadata.ino)
    ) {
      throw new Error(`${label} changed while it was being inspected: ${path}`)
    }
    return { bytes: await handle.readFile(), metadata: opened }
  } finally {
    await handle.close()
  }
}
