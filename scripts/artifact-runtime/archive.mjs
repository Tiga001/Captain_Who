/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { createHash, randomUUID } from 'node:crypto'
import { spawn } from 'node:child_process'
import {
  chmod,
  cp,
  lstat,
  mkdir,
  open,
  readFile,
  readdir,
  rename,
  rm,
  unlink,
  writeFile
} from 'node:fs/promises'
import { get as httpsGet } from 'node:https'
import { basename, dirname, join } from 'node:path'
import { inflateRawSync } from 'node:zlib'
import { extract } from 'tar'

import {
  ARTIFACT_RUNTIME_MAX_DOWNLOAD_BYTES,
  ARTIFACT_RUNTIME_MAX_REDIRECTS,
  SAFE_ENVIRONMENT,
  validateArtifactRuntimeDownloadUrl
} from './contract.mjs'
import { fileMatches, readRegularFileNoFollow } from './filesystem.mjs'

async function requestDownload(url, redirectsRemaining = ARTIFACT_RUNTIME_MAX_REDIRECTS) {
  validateArtifactRuntimeDownloadUrl(url)
  return new Promise((resolvePromise, rejectPromise) => {
    const request = httpsGet(
      url,
      {
        headers: {
          Accept: 'application/octet-stream',
          'User-Agent': 'CaptainWho-Artifact-Runtime-Preparer/1'
        }
      },
      (response) => {
        const status = response.statusCode ?? 0
        if ([301, 302, 303, 307, 308].includes(status)) {
          const location = response.headers.location
          response.resume()
          if (!location || redirectsRemaining === 0) {
            rejectPromise(new Error('Artifact runtime download redirect is invalid or excessive'))
            return
          }
          try {
            const redirected = new URL(location, url)
            validateArtifactRuntimeDownloadUrl(redirected.href, 'redirect URL')
            requestDownload(redirected.href, redirectsRemaining - 1).then(
              resolvePromise,
              rejectPromise
            )
          } catch (error) {
            rejectPromise(error)
          }
          return
        }
        if (status !== 200) {
          response.resume()
          rejectPromise(new Error(`Artifact runtime download failed with HTTP ${status}`))
          return
        }
        resolvePromise(response)
      }
    )
    request.setTimeout(60_000, () =>
      request.destroy(new Error('Artifact runtime download timed out'))
    )
    request.on('error', rejectPromise)
  })
}

export async function syncDirectory(path) {
  if (process.platform === 'win32') return
  const directory = await open(path, 'r')
  try {
    await directory.sync()
  } finally {
    await directory.close()
  }
}

export async function unlinkIfPresent(path) {
  try {
    await unlink(path)
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }
}

export async function downloadPinnedFile(descriptor, destination) {
  if (await fileMatches(destination, descriptor)) return destination
  await mkdir(dirname(destination), { recursive: true })
  const temporary = join(
    dirname(destination),
    `.${basename(destination)}.${process.pid}.${randomUUID()}.download`
  )
  let handle
  try {
    const response = await requestDownload(descriptor.url)
    const contentLength = response.headers['content-length']
    if (contentLength !== undefined && Number(contentLength) !== descriptor.size) {
      response.destroy()
      throw new Error(`Pinned download Content-Length does not match ${descriptor.size}`)
    }
    handle = await open(temporary, 'wx', 0o600)
    const digest = createHash('sha256')
    let size = 0
    for await (const chunk of response) {
      size += chunk.length
      if (size > descriptor.size || size > ARTIFACT_RUNTIME_MAX_DOWNLOAD_BYTES) {
        response.destroy()
        throw new Error('Artifact runtime download exceeded its pinned size')
      }
      digest.update(chunk)
      await handle.write(chunk)
    }
    if (size !== descriptor.size || digest.digest('hex') !== descriptor.sha256) {
      throw new Error('Artifact runtime download failed size or SHA-256 verification')
    }
    await handle.sync()
    await handle.close()
    handle = undefined
    await rename(temporary, destination)
    await syncDirectory(dirname(destination))
    return destination
  } finally {
    await handle?.close().catch(() => undefined)
    await unlinkIfPresent(temporary)
  }
}

export function archiveCachePath(downloadDirectory, descriptor) {
  const url = new URL(descriptor.url)
  const name = basename(decodeURIComponent(url.pathname))
  return join(downloadDirectory, `${descriptor.sha256.slice(0, 16)}-${name}`)
}

export async function runProcess(executable, args, options = {}) {
  const timeoutMs = options.timeoutMs ?? 600_000
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(executable, args, {
      cwd: options.cwd,
      env: { ...process.env, ...SAFE_ENVIRONMENT, ...options.env },
      shell: false,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true
    })
    const stdout = []
    const stderr = []
    let stdoutBytes = 0
    let stderrBytes = 0
    const outputLimit = 2 * 1024 * 1024
    child.stdout.on('data', (chunk) => {
      stdoutBytes += chunk.length
      if (stdoutBytes <= outputLimit) stdout.push(chunk)
    })
    child.stderr.on('data', (chunk) => {
      stderrBytes += chunk.length
      if (stderrBytes <= outputLimit) stderr.push(chunk)
    })
    const timer = setTimeout(() => child.kill('SIGKILL'), timeoutMs)
    child.once('error', (error) => {
      clearTimeout(timer)
      rejectPromise(error)
    })
    child.once('close', (code, signal) => {
      clearTimeout(timer)
      const result = {
        code,
        signal,
        stdout: Buffer.concat(stdout).toString('utf8'),
        stderr: Buffer.concat(stderr).toString('utf8')
      }
      if (code !== 0) {
        rejectPromise(
          new Error(
            `Process ${basename(executable)} failed with ${code ?? signal}: ${result.stderr.trim()}`
          )
        )
      } else {
        resolvePromise(result)
      }
    })
  })
}

export async function copyWithoutSymlinks(source, destination, { destinationExists = false } = {}) {
  await cp(source, destination, {
    recursive: true,
    dereference: true,
    errorOnExist: !destinationExists,
    force: destinationExists,
    preserveTimestamps: true
  })
}

export async function copyTreeRejectingSymlinks(
  source,
  destination,
  { destinationExists = false } = {}
) {
  const rootMetadata = await lstat(source)
  if (!rootMetadata.isDirectory() || rootMetadata.isSymbolicLink()) {
    throw new Error('Artifact runtime offline source must be a real, non-symlink directory')
  }
  if (!destinationExists) await mkdir(destination, { recursive: false, mode: 0o700 })
  const pending = [{ source, destination }]
  while (pending.length > 0) {
    const current = pending.pop()
    const entries = await readdir(current.source, { withFileTypes: true })
    for (const entry of entries) {
      const childSource = join(current.source, entry.name)
      const childDestination = join(current.destination, entry.name)
      const metadata = await lstat(childSource)
      if (metadata.isSymbolicLink()) {
        throw new Error(
          `Artifact runtime offline source contains a forbidden symlink: ${childSource}`
        )
      }
      if (metadata.isDirectory()) {
        await mkdir(childDestination, { recursive: false, mode: 0o700 })
        pending.push({ source: childSource, destination: childDestination })
      } else if (metadata.isFile()) {
        const { bytes, metadata: opened } = await readRegularFileNoFollow(
          childSource,
          'Artifact runtime offline source file'
        )
        await writeFile(childDestination, bytes, {
          flag: 'wx',
          mode: (opened.mode & 0o111) !== 0 ? 0o755 : 0o644
        })
      } else {
        throw new Error(
          `Artifact runtime offline source contains a non-regular entry: ${childSource}`
        )
      }
    }
  }
}

export async function acquireNodeRuntime(manifest, asset, staging, downloadDirectory) {
  const executableRelative =
    process.platform === 'win32' ? manifest.node.executable.win32 : manifest.node.executable.unix
  const executable = join(staging, ...executableRelative.split('/'))
  await mkdir(dirname(executable), { recursive: true })
  const archive = await downloadPinnedFile(asset, archiveCachePath(downloadDirectory, asset))
  if (asset.format === 'executable') {
    await cp(archive, executable, { force: false, errorOnExist: true })
  } else {
    const extraction = join(staging, `.node-extract-${randomUUID()}`)
    await mkdir(extraction, { recursive: true })
    try {
      await extract({ file: archive, cwd: extraction, strict: true, preservePaths: false })
      const source = join(extraction, asset.archiveRoot, 'bin', 'node')
      await cp(source, executable, { force: false, errorOnExist: true })
    } finally {
      await rm(extraction, { recursive: true, force: true })
    }
  }
  if (process.platform !== 'win32') await chmod(executable, 0o755)
  const result = await runProcess(executable, ['--version'], { timeoutMs: 15_000 })
  if (result.stdout.trim() !== `v${manifest.node.version}`) {
    throw new Error(`Managed Node probe returned unexpected version: ${result.stdout.trim()}`)
  }
  return executable
}

export function isPinnedRipgrepVersion(stdout, version) {
  const prefix = `ripgrep ${version}`
  if (!stdout.startsWith(prefix)) return false
  const boundary = stdout[prefix.length]
  return boundary === '\n' || boundary === '\r' || boundary === ' ' || boundary === '('
}

export function readPinnedZipMembers(bytes, requestedNames) {
  const minimumEocdBytes = 22
  const maximumCommentBytes = 65_535
  const firstCandidate = Math.max(0, bytes.length - minimumEocdBytes - maximumCommentBytes)
  let eocdOffset = -1
  for (let offset = bytes.length - minimumEocdBytes; offset >= firstCandidate; offset -= 1) {
    if (bytes.readUInt32LE(offset) === 0x06054b50) {
      eocdOffset = offset
      break
    }
  }
  if (eocdOffset < 0) throw new Error('Pinned ripgrep ZIP has no end-of-central-directory record')
  const disk = bytes.readUInt16LE(eocdOffset + 4)
  const centralDisk = bytes.readUInt16LE(eocdOffset + 6)
  const entriesOnDisk = bytes.readUInt16LE(eocdOffset + 8)
  const entryCount = bytes.readUInt16LE(eocdOffset + 10)
  const centralBytes = bytes.readUInt32LE(eocdOffset + 12)
  const centralOffset = bytes.readUInt32LE(eocdOffset + 16)
  const commentBytes = bytes.readUInt16LE(eocdOffset + 20)
  if (
    disk !== 0 ||
    centralDisk !== 0 ||
    entriesOnDisk !== entryCount ||
    entryCount === 0xffff ||
    centralBytes === 0xffffffff ||
    centralOffset === 0xffffffff ||
    eocdOffset + minimumEocdBytes + commentBytes !== bytes.length ||
    centralOffset + centralBytes > eocdOffset
  ) {
    throw new Error('Pinned ripgrep ZIP uses an unsupported split or ZIP64 layout')
  }

  const requested = new Set(requestedNames)
  const found = new Map()
  let offset = centralOffset
  for (let index = 0; index < entryCount; index += 1) {
    if (offset + 46 > bytes.length || bytes.readUInt32LE(offset) !== 0x02014b50) {
      throw new Error('Pinned ripgrep ZIP central directory is malformed')
    }
    const flags = bytes.readUInt16LE(offset + 8)
    const compression = bytes.readUInt16LE(offset + 10)
    const compressedBytes = bytes.readUInt32LE(offset + 20)
    const uncompressedBytes = bytes.readUInt32LE(offset + 24)
    const nameBytes = bytes.readUInt16LE(offset + 28)
    const extraBytes = bytes.readUInt16LE(offset + 30)
    const entryCommentBytes = bytes.readUInt16LE(offset + 32)
    const localOffset = bytes.readUInt32LE(offset + 42)
    const end = offset + 46 + nameBytes + extraBytes + entryCommentBytes
    if (end > centralOffset + centralBytes || flags & 0x1) {
      throw new Error('Pinned ripgrep ZIP contains an encrypted or malformed entry')
    }
    const name = bytes.subarray(offset + 46, offset + 46 + nameBytes).toString('utf8')
    if (requested.has(name)) {
      if (found.has(name)) throw new Error(`Pinned ripgrep ZIP repeats required entry ${name}`)
      if (
        localOffset + 30 > bytes.length ||
        bytes.readUInt32LE(localOffset) !== 0x04034b50 ||
        bytes.readUInt16LE(localOffset + 8) !== compression
      ) {
        throw new Error(`Pinned ripgrep ZIP local header is invalid for ${name}`)
      }
      const localNameBytes = bytes.readUInt16LE(localOffset + 26)
      const localExtraBytes = bytes.readUInt16LE(localOffset + 28)
      const dataOffset = localOffset + 30 + localNameBytes + localExtraBytes
      const dataEnd = dataOffset + compressedBytes
      if (dataEnd > bytes.length || uncompressedBytes > ARTIFACT_RUNTIME_MAX_DOWNLOAD_BYTES) {
        throw new Error(`Pinned ripgrep ZIP entry is oversized or truncated: ${name}`)
      }
      const compressed = bytes.subarray(dataOffset, dataEnd)
      const content =
        compression === 0
          ? Buffer.from(compressed)
          : compression === 8
            ? inflateRawSync(compressed, { maxOutputLength: uncompressedBytes })
            : null
      if (!content || content.length !== uncompressedBytes) {
        throw new Error(`Pinned ripgrep ZIP entry has unsupported or invalid compression: ${name}`)
      }
      found.set(name, content)
    }
    offset = end
  }
  if (offset !== centralOffset + centralBytes) {
    throw new Error('Pinned ripgrep ZIP central directory length does not match its record')
  }
  for (const name of requested) {
    if (!found.has(name)) throw new Error(`Pinned ripgrep archive is missing ${name}`)
  }
  return found
}

export async function acquireRipgrep(manifest, asset, staging, downloadDirectory) {
  const archive = await downloadPinnedFile(asset, archiveCachePath(downloadDirectory, asset))
  const executableRelative =
    process.platform === 'win32'
      ? manifest.tools.ripgrep.executable.win32
      : manifest.tools.ripgrep.executable.unix
  const members = [
    { source: process.platform === 'win32' ? 'rg.exe' : 'rg', target: executableRelative },
    ...manifest.tools.ripgrep.licenseFiles
  ]
  const requiredNames = members.map(({ source }) => `${asset.archiveRoot}/${source}`)
  const extracted = new Map()
  if (asset.format === 'zip') {
    const archiveBytes = await readFile(archive)
    for (const [name, content] of readPinnedZipMembers(archiveBytes, requiredNames)) {
      extracted.set(name, content)
    }
  } else {
    const extraction = join(staging, `.ripgrep-extract-${randomUUID()}`)
    await mkdir(extraction, { recursive: false, mode: 0o700 })
    try {
      await extract({ file: archive, cwd: extraction, strict: true, preservePaths: false })
      for (const name of requiredNames) {
        const { bytes } = await readRegularFileNoFollow(
          join(extraction, ...name.split('/')),
          `Pinned ripgrep archive member ${name}`
        )
        extracted.set(name, bytes)
      }
    } finally {
      await rm(extraction, { recursive: true, force: true })
    }
  }

  for (const member of members) {
    const source = `${asset.archiveRoot}/${member.source}`
    const destination = join(staging, ...member.target.split('/'))
    await mkdir(dirname(destination), { recursive: true })
    await writeFile(destination, extracted.get(source), {
      flag: 'wx',
      mode: member.target === executableRelative ? 0o755 : 0o644
    })
  }
  const executable = join(staging, ...executableRelative.split('/'))
  if (process.platform !== 'win32') await chmod(executable, 0o755)
  const result = await runProcess(executable, ['--version'], { timeoutMs: 15_000 })
  if (!isPinnedRipgrepVersion(result.stdout, manifest.tools.ripgrep.version)) {
    throw new Error(`Managed ripgrep probe returned unexpected version: ${result.stdout.trim()}`)
  }
  return executable
}
