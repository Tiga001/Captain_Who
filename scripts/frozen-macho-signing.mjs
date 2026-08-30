/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { constants as fsConstants } from 'node:fs'
import { open } from 'node:fs/promises'
import { resolve } from 'node:path'

const MACH_O_MAGICS = new Set([
  'cafebabe',
  'cafebabf',
  'bebafeca',
  'bfbafeca',
  'cefaedfe',
  'cffaedfe',
  'feedface',
  'feedfacf'
])

function nonEmptyString(value, label) {
  if (typeof value !== 'string' || value.length === 0 || value.trim() !== value) {
    throw new Error(`${label} must be a non-empty, trimmed string`)
  }
  return value
}

export function canonicalFrozenComponentPath(value, label = 'component path') {
  const path = nonEmptyString(value, label)
  if (
    path.length > 1024 ||
    path.includes('\\') ||
    path.includes('\0') ||
    path.startsWith('/') ||
    path.split('/').some((part) => part.length === 0 || part === '.' || part === '..')
  ) {
    throw new Error(`${label} must be a canonical relative path`)
  }
  return path
}

async function readMagicNoFollow(filePath, label) {
  const flags =
    fsConstants.O_RDONLY | (typeof fsConstants.O_NOFOLLOW === 'number' ? fsConstants.O_NOFOLLOW : 0)
  const handle = await open(filePath, flags)
  try {
    const metadata = await handle.stat()
    if (!metadata.isFile()) {
      throw new Error(`${label} must be a regular non-symlink file`)
    }
    const magic = Buffer.alloc(4)
    const { bytesRead } = await handle.read(magic, 0, magic.length, 0)
    return bytesRead === magic.length ? magic.toString('hex') : undefined
  } finally {
    await handle.close()
  }
}

export async function isMachOFile(filePath, label = 'component file') {
  try {
    return MACH_O_MAGICS.has(await readMagicNoFollow(filePath, label))
  } catch (error) {
    if (error?.code === 'ELOOP') {
      throw new Error(`${label} must be a regular non-symlink file`, { cause: error })
    }
    throw error
  }
}

export async function collectFrozenMachOTargets({
  outputDirectory,
  files,
  pathKey,
  label = 'Frozen component'
}) {
  if (typeof outputDirectory !== 'string' || outputDirectory.length === 0) {
    throw new Error(`${label} output directory is required`)
  }
  if (!Array.isArray(files) || files.length === 0) {
    throw new Error(`${label} receipt must contain a frozen file set`)
  }
  if (pathKey !== 'path' && pathKey !== 'name') {
    throw new Error(`${label} receipt path key must be path or name`)
  }

  const seen = new Set()
  const targets = []
  for (const [index, descriptor] of files.entries()) {
    if (!descriptor || typeof descriptor !== 'object' || Array.isArray(descriptor)) {
      throw new Error(`${label} receipt file ${index} must be an object`)
    }
    const relativePath = canonicalFrozenComponentPath(
      descriptor[pathKey],
      `${label} receipt file ${index}.${pathKey}`
    )
    if (seen.has(relativePath)) {
      throw new Error(`${label} receipt file paths must be unique`)
    }
    seen.add(relativePath)
    const path = resolve(outputDirectory, ...relativePath.split('/'))
    if (await isMachOFile(path, `${label} file ${relativePath}`)) {
      targets.push(Object.freeze({ relativePath, path }))
    }
  }

  targets.sort((left, right) =>
    Buffer.from(left.relativePath).compare(Buffer.from(right.relativePath))
  )
  return Object.freeze(targets)
}

export function assertExactMachOSigningPaths({ signedPaths, targets, label = 'Frozen component' }) {
  if (!Array.isArray(signedPaths) || signedPaths.length === 0) {
    throw new Error(`${label} signing must declare at least one mutated Mach-O path`)
  }
  if (!Array.isArray(targets) || targets.length === 0) {
    throw new Error(`${label} does not contain any frozen Mach-O files`)
  }
  const declared = signedPaths.map((path, index) =>
    canonicalFrozenComponentPath(path, `${label} signedPaths[${index}]`)
  )
  if (new Set(declared).size !== declared.length) {
    throw new Error(`${label} signing paths must be unique`)
  }
  const expected = targets.map(({ relativePath }) => relativePath)
  declared.sort((left, right) => Buffer.from(left).compare(Buffer.from(right)))
  expected.sort((left, right) => Buffer.from(left).compare(Buffer.from(right)))
  if (
    declared.length !== expected.length ||
    declared.some((path, index) => path !== expected[index])
  ) {
    throw new Error(`${label} signing paths must exactly match every frozen Mach-O file`)
  }
  return Object.freeze(declared)
}

export function assertOnlyFrozenMachOFilesChanged({
  beforeFiles,
  afterFiles,
  signedPaths,
  pathKey,
  label = 'Frozen component'
}) {
  if (!Array.isArray(beforeFiles) || !Array.isArray(afterFiles)) {
    throw new Error(`${label} signing comparison requires file descriptor arrays`)
  }
  if (pathKey !== 'path' && pathKey !== 'name') {
    throw new Error(`${label} receipt path key must be path or name`)
  }
  if (
    beforeFiles.length !== afterFiles.length ||
    beforeFiles.some((file, index) => file[pathKey] !== afterFiles[index]?.[pathKey])
  ) {
    throw new Error(`${label} file set changed during the signing transaction`)
  }

  const allowed = new Set(signedPaths)
  const changed = new Set()
  for (let index = 0; index < beforeFiles.length; index += 1) {
    const before = beforeFiles[index]
    const after = afterFiles[index]
    if (before.size !== after.size || before.sha256 !== after.sha256) {
      changed.add(before[pathKey])
    }
  }
  for (const path of changed) {
    if (!allowed.has(path)) {
      throw new Error(`${label} file outside the Mach-O signing allowlist changed: ${path}`)
    }
  }
  for (const path of allowed) {
    if (!changed.has(path)) {
      throw new Error(`${label} signing did not change expected Mach-O: ${path}`)
    }
  }
  return Object.freeze([...changed])
}
