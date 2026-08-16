/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { spawn } from 'node:child_process'
import { createHash, randomUUID } from 'node:crypto'
import { createReadStream, createWriteStream } from 'node:fs'
import {
  chmod,
  cp,
  lstat,
  mkdtemp,
  mkdir,
  open,
  readFile,
  readlink,
  readdir,
  realpath,
  rename,
  rm
} from 'node:fs/promises'
import { basename, dirname, join, posix as pathPosix, resolve, sep } from 'node:path'
import { tmpdir } from 'node:os'
import { Readable, Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'

const RECEIPT_NAME = 'component-receipt.json'
const PROVIDER_ID = 'mycopilot.word-pdf-render-runtime'
const BUNDLE_VERSION = '2026.08.1'
const LIBREOFFICE_VERSION = '26.2.4.2'
const REVISION_PREFIX = 'word-pdf-render-runtime-sha256-v1:'
const MAX_FILES = 20_000
const MAX_TOTAL_BYTES = 2 * 1024 * 1024 * 1024
const MAX_FILE_BYTES = 512 * 1024 * 1024
const MAX_ARCHIVE_BYTES = 512 * 1024 * 1024
const MAX_RECEIPT_BYTES = 8 * 1024 * 1024
const SHA256_PATTERN = /^[a-f0-9]{64}$/
const SCRIPT_DIRECTORY = dirname(fileURLToPath(import.meta.url))
const REPOSITORY_ROOT = resolve(SCRIPT_DIRECTORY, '..')
const DEFAULT_MANIFEST_PATH = join(REPOSITORY_ROOT, 'resources', 'word-pdf-renderer-manifest.json')
const DEFAULT_OUTPUT_DIRECTORY = join(REPOSITORY_ROOT, '.cache', 'word-pdf-renderer', 'current')

function plainObject(value, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be an object`)
  }
  return value
}

function exactKeys(value, expected, label) {
  const actual = Object.keys(value).sort()
  const frozen = [...expected].sort()
  if (actual.length !== frozen.length || actual.some((key, index) => key !== frozen[index])) {
    throw new Error(`${label} must contain exactly: ${frozen.join(', ')}`)
  }
}

function nonEmptyString(value, label) {
  if (typeof value !== 'string' || value.length === 0 || value.trim() !== value) {
    throw new Error(`${label} must be a non-empty, trimmed string`)
  }
  return value
}

function canonicalRelativePath(value, label) {
  const path = nonEmptyString(value, label)
  if (
    path.length > 1024 ||
    path.includes('\\') ||
    path.includes('\0') ||
    path.startsWith('/') ||
    path.split('/').some((part) => part.length === 0 || part === '.' || part === '..')
  ) {
    throw new Error(`${label} must be a canonical POSIX-relative path`)
  }
  return path
}

function pinnedArchive(value, label) {
  const archive = plainObject(value, label)
  exactKeys(archive, ['url', 'size', 'sha256'], label)
  const url = new URL(nonEmptyString(archive.url, `${label}.url`))
  if (
    url.protocol !== 'https:' ||
    url.hostname !== 'downloadarchive.documentfoundation.org' ||
    url.username ||
    url.password ||
    url.port ||
    url.search ||
    url.hash ||
    !url.pathname.startsWith(`/libreoffice/old/${LIBREOFFICE_VERSION}/`)
  ) {
    throw new Error(`${label}.url must be an exact LibreOffice archive URL`)
  }
  if (
    !Number.isSafeInteger(archive.size) ||
    archive.size <= 0 ||
    archive.size > MAX_ARCHIVE_BYTES
  ) {
    throw new Error(`${label}.size must be a positive bounded integer`)
  }
  if (!SHA256_PATTERN.test(archive.sha256)) {
    throw new Error(`${label}.sha256 must be a lowercase SHA-256 digest`)
  }
  return Object.freeze({ url: url.toString(), size: archive.size, sha256: archive.sha256 })
}

function canonicalValue(value) {
  if (Array.isArray(value)) return value.map(canonicalValue)
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, canonicalValue(value[key])])
    )
  }
  return value
}

export function computeWordPdfRendererRevision(receiptFields) {
  const value = plainObject(receiptFields, 'receipt fields')
  const payload = Object.fromEntries(
    Object.entries(value).filter(([key]) => key !== 'bundleRevision')
  )
  const digest = createHash('sha256')
    .update(JSON.stringify(canonicalValue(payload)))
    .digest('hex')
  return `${REVISION_PREFIX}${digest}`
}

export function validateWordPdfRendererManifest(value) {
  const manifest = plainObject(value, 'manifest')
  exactKeys(
    manifest,
    ['schemaVersion', 'providerId', 'bundleVersion', 'libreOffice', 'targets'],
    'manifest'
  )
  if (manifest.schemaVersion !== 1) throw new Error('manifest.schemaVersion must be 1')
  if (manifest.providerId !== PROVIDER_ID)
    throw new Error(`manifest.providerId must be ${PROVIDER_ID}`)
  if (manifest.bundleVersion !== BUNDLE_VERSION) {
    throw new Error(`Word PDF renderer bundle must remain pinned to ${BUNDLE_VERSION}`)
  }
  const libreOffice = plainObject(manifest.libreOffice, 'manifest.libreOffice')
  exactKeys(libreOffice, ['family', 'version'], 'manifest.libreOffice')
  if (libreOffice.family !== 'libreoffice' || libreOffice.version !== LIBREOFFICE_VERSION) {
    throw new Error(`Word PDF renderer must pin LibreOffice ${LIBREOFFICE_VERSION}`)
  }

  const targets = plainObject(manifest.targets, 'manifest.targets')
  const targetDefinitions = {
    'darwin-arm64': ['dmg', 'LibreOffice.app'],
    'darwin-x64': ['dmg', 'LibreOffice.app']
  }
  exactKeys(targets, Object.keys(targetDefinitions), 'manifest.targets')
  const validatedTargets = Object.fromEntries(
    Object.entries(targets).map(([key, raw]) => {
      const target = plainObject(raw, `manifest.targets.${key}`)
      exactKeys(
        target,
        ['archiveFormat', 'payload', 'executable', 'license', 'notice', 'archive'],
        `manifest.targets.${key}`
      )
      const [archiveFormat, payload] = targetDefinitions[key]
      if (target.archiveFormat !== archiveFormat || target.payload !== payload) {
        throw new Error(`manifest.targets.${key} must use its pinned LibreOffice archive layout`)
      }
      const executable = canonicalRelativePath(
        target.executable,
        `manifest.targets.${key}.executable`
      )
      const license = canonicalRelativePath(target.license, `manifest.targets.${key}.license`)
      const notice = canonicalRelativePath(target.notice, `manifest.targets.${key}.notice`)
      if (![executable, license, notice].every((path) => path.startsWith('libreoffice/'))) {
        throw new Error(`manifest.targets.${key} paths must remain inside libreoffice/`)
      }
      return [
        key,
        Object.freeze({
          archiveFormat,
          payload,
          executable,
          license,
          notice,
          archive: pinnedArchive(target.archive, `manifest.targets.${key}.archive`)
        })
      ]
    })
  )
  return Object.freeze({
    schemaVersion: 1,
    providerId: PROVIDER_ID,
    bundleVersion: BUNDLE_VERSION,
    libreOffice: Object.freeze({ ...libreOffice }),
    targets: Object.freeze(validatedTargets)
  })
}

export async function loadWordPdfRendererManifest(manifestPath = DEFAULT_MANIFEST_PATH) {
  let parsed
  try {
    parsed = JSON.parse(await readFile(manifestPath, 'utf8'))
  } catch (error) {
    throw new Error(`Word PDF renderer manifest is not valid JSON: ${error.message}`, {
      cause: error
    })
  }
  return validateWordPdfRendererManifest(parsed)
}

export function selectWordPdfRendererTarget(
  manifest,
  platform = process.platform,
  arch = process.arch
) {
  const target = manifest.targets[`${platform}-${arch}`]
  if (!target) {
    throw new Error(
      `Word PDF renderer ${LIBREOFFICE_VERSION} is not packaged for ${platform}-${arch}`
    )
  }
  return target
}

async function hashFile(path) {
  const digest = createHash('sha256')
  for await (const chunk of createReadStream(path)) digest.update(chunk)
  return digest.digest('hex')
}

async function syncDirectory(path) {
  if (process.platform === 'win32') return
  const handle = await open(path, 'r')
  try {
    await handle.sync()
  } finally {
    await handle.close()
  }
}

async function inspectComponentTree(root) {
  const rootMetadata = await lstat(root)
  if (!rootMetadata.isDirectory() || rootMetadata.isSymbolicLink()) {
    throw new Error('Word PDF renderer root must be a real, non-symlink directory')
  }
  const files = []
  const links = []
  let totalBytes = 0
  const canonicalRoot = await realpathDirectory(root)
  const pending = [{ directory: root, prefix: '' }]
  while (pending.length > 0) {
    const current = pending.pop()
    for (const entry of await readdir(current.directory, { withFileTypes: true })) {
      const path = join(current.directory, entry.name)
      const logical = current.prefix ? `${current.prefix}/${entry.name}` : entry.name
      const metadata = await lstat(path)
      if (metadata.isSymbolicLink()) {
        if (files.length + links.length >= MAX_FILES) {
          throw new Error('Word PDF renderer exceeds its entry-count limit')
        }
        const target = await readlink(path)
        validateSafeLink(logical, target)
        const resolved = await realpath(path).catch(() => undefined)
        if (
          !resolved ||
          (resolved !== canonicalRoot && !resolved.startsWith(`${canonicalRoot}${sep}`))
        ) {
          throw new Error(`Word PDF renderer symlink escapes its component root: ${logical}`)
        }
        links.push({ path: logical, target })
        continue
      }
      if (metadata.isDirectory()) {
        pending.push({ directory: path, prefix: logical })
        continue
      }
      if (!metadata.isFile()) {
        throw new Error(`Word PDF renderer entry is not a regular file: ${logical}`)
      }
      if (logical === RECEIPT_NAME) continue
      if (files.length + links.length >= MAX_FILES) {
        throw new Error('Word PDF renderer exceeds its entry-count limit')
      }
      if (metadata.size > MAX_FILE_BYTES) {
        throw new Error(`Word PDF renderer file exceeds its byte limit: ${logical}`)
      }
      totalBytes += metadata.size
      if (totalBytes > MAX_TOTAL_BYTES)
        throw new Error('Word PDF renderer exceeds its total byte limit')
      files.push({ path: logical, size: metadata.size, sha256: await hashFile(path) })
    }
  }
  files.sort((left, right) => Buffer.from(left.path).compare(Buffer.from(right.path)))
  links.sort((left, right) => Buffer.from(left.path).compare(Buffer.from(right.path)))
  return { files, links }
}

async function realpathDirectory(path) {
  const canonical = await realpath(path)
  const metadata = await lstat(canonical)
  if (!metadata.isDirectory()) throw new Error(`Expected a directory: ${path}`)
  return canonical
}

function validateSafeLink(path, target) {
  canonicalRelativePath(path, 'Word PDF renderer symlink path')
  const value = nonEmptyString(target, `Word PDF renderer symlink ${path} target`)
  if (value.includes('\\') || value.includes('\0') || pathPosix.isAbsolute(value)) {
    throw new Error(`Word PDF renderer symlink target must be relative: ${path}`)
  }
  const resolved = pathPosix.normalize(pathPosix.join(pathPosix.dirname(path), value))
  if (
    resolved === '.' ||
    resolved === '..' ||
    resolved.startsWith('../') ||
    pathPosix.isAbsolute(resolved)
  ) {
    throw new Error(`Word PDF renderer symlink target escapes its component root: ${path}`)
  }
  return value
}

async function runProcess(executable, args, { timeoutMs = 600_000, env = process.env } = {}) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(executable, args, {
      env,
      shell: false,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true
    })
    const stdout = []
    const stderr = []
    let settled = false
    const finish = (callback) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      callback()
    }
    child.stdout.on('data', (chunk) => {
      if (stdout.reduce((sum, value) => sum + value.length, 0) < 2 * 1024 * 1024) stdout.push(chunk)
    })
    child.stderr.on('data', (chunk) => {
      if (stderr.reduce((sum, value) => sum + value.length, 0) < 2 * 1024 * 1024) stderr.push(chunk)
    })
    child.once('error', (error) => finish(() => rejectPromise(error)))
    child.once('close', (code, signal) =>
      finish(() => {
        const result = {
          code,
          signal,
          stdout: Buffer.concat(stdout).toString('utf8'),
          stderr: Buffer.concat(stderr).toString('utf8')
        }
        if (code === 0) resolvePromise(result)
        else
          rejectPromise(
            new Error(
              `${basename(executable)} failed with ${code ?? signal}: ${result.stderr.trim()}`
            )
          )
      })
    )
    const timer = setTimeout(() => {
      child.kill('SIGKILL')
      finish(() => rejectPromise(new Error(`${basename(executable)} timed out`)))
    }, timeoutMs)
  })
}

export async function downloadPinnedWordPdfRendererArchive(archive, outputPath, fetchImpl = fetch) {
  // The Document Foundation archive uses MirrorBrain redirects. The immutable size and SHA-256
  // below, rather than a mutable mirror hostname, remain the artifact trust boundary.
  const response = await fetchImpl(archive.url, { redirect: 'follow' })
  if (!response.ok || !response.body) {
    throw new Error(`Pinned LibreOffice archive returned HTTP ${response.status}`)
  }
  const finalUrl = new URL(response.url || archive.url)
  if (finalUrl.protocol !== 'https:') {
    throw new Error('Pinned LibreOffice archive redirected outside HTTPS')
  }
  const contentLength = response.headers.get('content-length')
  if (contentLength !== null && Number(contentLength) !== archive.size) {
    throw new Error('Pinned LibreOffice archive Content-Length does not match the manifest')
  }
  const digest = createHash('sha256')
  let bytes = 0
  const verifier = new Transform({
    transform(chunk, _encoding, callback) {
      bytes += chunk.length
      if (bytes > archive.size || bytes > MAX_ARCHIVE_BYTES) {
        callback(new Error('Pinned LibreOffice archive exceeds its frozen byte limit'))
        return
      }
      digest.update(chunk)
      callback(null, chunk)
    }
  })
  await pipeline(
    Readable.fromWeb(response.body),
    verifier,
    createWriteStream(outputPath, { flags: 'wx', mode: 0o600 })
  )
  if (bytes !== archive.size || digest.digest('hex') !== archive.sha256) {
    throw new Error('Pinned LibreOffice archive failed its frozen size or SHA-256 check')
  }
}

export async function installPinnedWordPdfRenderer({ installRoot, platform, arch, target }) {
  if (platform !== 'darwin' || platform !== process.platform || arch !== process.arch) {
    throw new Error(
      `LibreOffice can prepare only the current host target (${process.platform}-${process.arch})`
    )
  }
  const archivePath = join(installRoot, 'libreoffice.dmg')
  await downloadPinnedWordPdfRendererArchive(target.archive, archivePath)
  try {
    return await installMacDmg(installRoot, archivePath, target)
  } finally {
    await rm(archivePath, { force: true })
  }
}

async function installMacDmg(installRoot, archivePath, target) {
  const mountPoint = join(installRoot, 'mount')
  const extracted = join(installRoot, 'extracted')
  await mkdir(mountPoint, { mode: 0o700 })
  await mkdir(join(extracted, 'libreoffice'), { recursive: true, mode: 0o700 })
  let mounted = false
  try {
    await runProcess('/usr/bin/hdiutil', [
      'attach',
      archivePath,
      '-readonly',
      '-nobrowse',
      '-mountpoint',
      mountPoint
    ])
    mounted = true
    const source = join(mountPoint, target.payload)
    const sourceMetadata = await lstat(source)
    if (!sourceMetadata.isDirectory() || sourceMetadata.isSymbolicLink()) {
      throw new Error(
        'Authenticated LibreOffice DMG does not contain the pinned application payload'
      )
    }
    await cp(source, join(extracted, 'libreoffice', target.payload), {
      recursive: true,
      dereference: false,
      errorOnExist: true,
      force: false,
      preserveTimestamps: true,
      verbatimSymlinks: true
    })
    await verifyWordPdfRendererCodeSignature(extracted, target)
  } finally {
    if (mounted) {
      await runProcess('/usr/bin/hdiutil', ['detach', mountPoint, '-force'], {
        timeoutMs: 60_000
      }).catch(() => undefined)
    }
    await rm(mountPoint, { recursive: true, force: true })
  }
  return extracted
}

export async function verifyWordPdfRendererCodeSignature(componentRoot, target) {
  const appRoot = join(componentRoot, 'libreoffice', target.payload)
  const metadata = await lstat(appRoot)
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new Error('Prepared LibreOffice application bundle is missing')
  }
  await runProcess('/usr/bin/codesign', ['--verify', '--deep', '--strict', appRoot], {
    timeoutMs: 120_000
  })
}

async function probeExecutable(path, expectedVersion) {
  const metadata = await lstat(path)
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error('Word PDF renderer executable must be a regular non-symlink file')
  }
  if (process.platform !== 'win32') await chmod(path, metadata.mode | 0o755)
  const isolation = await mkdtemp(join(tmpdir(), 'mycopilot-word-pdf-probe-'))
  const home = join(isolation, 'home')
  const profile = join(isolation, 'profile')
  const cache = join(isolation, 'cache')
  await Promise.all([
    mkdir(home, { mode: 0o700 }),
    mkdir(profile, { mode: 0o700 }),
    mkdir(cache, { mode: 0o700 })
  ])
  let result
  try {
    result = await runProcess(
      path,
      [
        `-env:UserInstallation=${pathToFileURL(profile).href}`,
        '--headless',
        '--nologo',
        '--nodefault',
        '--nolockcheck',
        '--norestore',
        '--version'
      ],
      {
        timeoutMs: 30_000,
        env: {
          ...process.env,
          HOME: home,
          TMPDIR: isolation,
          XDG_CACHE_HOME: cache,
          XDG_CONFIG_HOME: join(isolation, 'config')
        }
      }
    )
  } finally {
    await rm(isolation, { recursive: true, force: true })
  }
  const reported = `${result.stdout}\n${result.stderr}`.trim()
  if (!reported.includes(expectedVersion)) {
    throw new Error(`Word PDF renderer reported an unexpected LibreOffice version: ${reported}`)
  }
}

function buildReceipt(manifest, target, platform, arch, files, links) {
  const payload = {
    schemaVersion: 1,
    providerId: manifest.providerId,
    bundleVersion: manifest.bundleVersion,
    platform,
    arch,
    runtime: {
      family: manifest.libreOffice.family,
      version: manifest.libreOffice.version,
      executable: target.executable,
      license: target.license,
      notice: target.notice
    },
    archive: target.archive,
    files,
    links
  }
  return { ...payload, bundleRevision: computeWordPdfRendererRevision(payload) }
}

function validateReceipt(value, manifest, target, platform, arch) {
  const receipt = plainObject(value, 'Word PDF renderer receipt')
  exactKeys(
    receipt,
    [
      'schemaVersion',
      'providerId',
      'bundleVersion',
      'platform',
      'arch',
      'runtime',
      'archive',
      'files',
      'links',
      'bundleRevision'
    ],
    'Word PDF renderer receipt'
  )
  if (
    receipt.schemaVersion !== 1 ||
    receipt.providerId !== manifest.providerId ||
    receipt.bundleVersion !== manifest.bundleVersion ||
    receipt.platform !== platform ||
    receipt.arch !== arch
  ) {
    throw new Error('Word PDF renderer receipt identity does not match this target')
  }
  const expectedRuntime = {
    family: manifest.libreOffice.family,
    version: manifest.libreOffice.version,
    executable: target.executable,
    license: target.license,
    notice: target.notice
  }
  if (JSON.stringify(receipt.runtime) !== JSON.stringify(expectedRuntime)) {
    throw new Error('Word PDF renderer runtime identity does not match the pinned manifest')
  }
  if (JSON.stringify(receipt.archive) !== JSON.stringify(target.archive)) {
    throw new Error('Word PDF renderer archive identity does not match the pinned manifest')
  }
  if (
    !Array.isArray(receipt.files) ||
    receipt.files.length < 3 ||
    receipt.files.length > MAX_FILES
  ) {
    throw new Error('Word PDF renderer receipt file set is invalid')
  }
  let prior
  let total = 0
  for (const [index, raw] of receipt.files.entries()) {
    const file = plainObject(raw, `Word PDF renderer receipt.files[${index}]`)
    exactKeys(file, ['path', 'size', 'sha256'], `Word PDF renderer receipt.files[${index}]`)
    const path = canonicalRelativePath(file.path, `Word PDF renderer receipt.files[${index}].path`)
    if (!path.startsWith('libreoffice/'))
      throw new Error('Word PDF renderer files must stay inside libreoffice/')
    if (prior !== undefined && Buffer.compare(Buffer.from(prior), Buffer.from(path)) >= 0) {
      throw new Error('Word PDF renderer receipt files must be uniquely sorted')
    }
    prior = path
    if (
      !Number.isSafeInteger(file.size) ||
      file.size < 0 ||
      file.size > MAX_FILE_BYTES ||
      !SHA256_PATTERN.test(file.sha256)
    ) {
      throw new Error('Word PDF renderer receipt contains an invalid file descriptor')
    }
    total += file.size
    if (total > MAX_TOTAL_BYTES) throw new Error('Word PDF renderer receipt exceeds its byte limit')
  }
  if (!Array.isArray(receipt.links) || receipt.links.length > MAX_FILES) {
    throw new Error('Word PDF renderer receipt symlink set is invalid')
  }
  prior = undefined
  for (const [index, raw] of receipt.links.entries()) {
    const link = plainObject(raw, `Word PDF renderer receipt.links[${index}]`)
    exactKeys(link, ['path', 'target'], `Word PDF renderer receipt.links[${index}]`)
    const path = canonicalRelativePath(link.path, `Word PDF renderer receipt.links[${index}].path`)
    validateSafeLink(path, link.target)
    if (!path.startsWith('libreoffice/')) {
      throw new Error('Word PDF renderer symlinks must stay inside libreoffice/')
    }
    if (prior !== undefined && Buffer.compare(Buffer.from(prior), Buffer.from(path)) >= 0) {
      throw new Error('Word PDF renderer receipt symlinks must be uniquely sorted')
    }
    if (receipt.files.some((file) => file.path === path)) {
      throw new Error('Word PDF renderer receipt path cannot be both a file and symlink')
    }
    prior = path
  }
  if (receipt.files.length + receipt.links.length > MAX_FILES) {
    throw new Error('Word PDF renderer receipt exceeds its entry-count limit')
  }
  for (const required of [target.executable, target.license, target.notice]) {
    const file = receipt.files.find(({ path }) => path === required)
    if (!file || file.size <= 0) throw new Error(`Word PDF renderer receipt is missing ${required}`)
  }
  if (receipt.bundleRevision !== computeWordPdfRendererRevision(receipt)) {
    throw new Error('Word PDF renderer bundle revision does not match its receipt')
  }
  return receipt
}

async function writeReceipt(root, receipt) {
  const handle = await open(join(root, RECEIPT_NAME), 'wx', 0o600)
  try {
    await handle.writeFile(`${JSON.stringify(receipt, null, 2)}\n`)
    await handle.sync()
  } finally {
    await handle.close()
  }
  if (process.platform !== 'win32') await chmod(join(root, RECEIPT_NAME), 0o644)
  await syncDirectory(root)
}

async function verifyReceipt(root, manifest, target, platform, arch) {
  const receiptPath = join(root, RECEIPT_NAME)
  const metadata = await lstat(receiptPath)
  if (
    !metadata.isFile() ||
    metadata.isSymbolicLink() ||
    metadata.size <= 0 ||
    metadata.size > MAX_RECEIPT_BYTES
  ) {
    throw new Error('Word PDF renderer receipt must be a small regular non-symlink file')
  }
  const bytes = await readFile(receiptPath)
  if (bytes.includes(0)) throw new Error('Word PDF renderer receipt contains a NUL byte')
  const receipt = validateReceipt(
    JSON.parse(bytes.toString('utf8')),
    manifest,
    target,
    platform,
    arch
  )
  const executable = join(root, ...target.executable.split('/'))
  const executableMetadata = await lstat(executable)
  if (!executableMetadata.isFile() || executableMetadata.isSymbolicLink()) {
    throw new Error('Word PDF renderer executable must be a regular non-symlink file')
  }
  if (process.platform !== 'win32' && (executableMetadata.mode & 0o111) === 0) {
    throw new Error('Word PDF renderer executable must have a Unix execute bit')
  }
  const actual = await inspectComponentTree(root)
  if (
    JSON.stringify(actual.files) !== JSON.stringify(receipt.files) ||
    JSON.stringify(actual.links) !== JSON.stringify(receipt.links)
  ) {
    throw new Error('Word PDF renderer files do not match the frozen receipt')
  }
  return receipt
}

async function publishDirectoryAtomically(staging, outputDirectory, hooks) {
  const parent = dirname(outputDirectory)
  const backup = join(parent, `.${basename(outputDirectory)}.${process.pid}.${randomUUID()}.backup`)
  let moved = false
  try {
    await hooks.beforePublish?.({ staging, outputDirectory })
    try {
      await rename(outputDirectory, backup)
      moved = true
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
    await rename(staging, outputDirectory)
    await syncDirectory(parent)
  } catch (error) {
    if (moved) await rename(backup, outputDirectory).catch(() => undefined)
    throw error
  }
  if (moved) await rm(backup, { recursive: true, force: true })
}

export async function prepareWordPdfRenderer({
  manifestPath = DEFAULT_MANIFEST_PATH,
  outputDirectory = DEFAULT_OUTPUT_DIRECTORY,
  platform = process.platform,
  arch = process.arch,
  verifyOnly = false,
  forceRebuild = false,
  installer = installPinnedWordPdfRenderer,
  probe = probeExecutable,
  hooks = {}
} = {}) {
  const manifest = await loadWordPdfRendererManifest(manifestPath)
  const target = selectWordPdfRendererTarget(manifest, platform, arch)
  if (verifyOnly) {
    const receipt = await verifyReceipt(outputDirectory, manifest, target, platform, arch)
    return Object.freeze({ outputDirectory, receipt, reused: true })
  }
  if (!forceRebuild) {
    try {
      const receipt = await verifyReceipt(outputDirectory, manifest, target, platform, arch)
      return Object.freeze({ outputDirectory, receipt, reused: true })
    } catch {
      // Rebuild only from the pinned, size- and hash-verified LibreOffice archive.
    }
  }
  const parent = dirname(outputDirectory)
  await mkdir(parent, { recursive: true })
  const staging = join(
    parent,
    `.${basename(outputDirectory)}.${process.pid}.${randomUUID()}.staging`
  )
  await mkdir(staging, { mode: 0o700 })
  try {
    const installRoot = join(staging, '.install')
    await mkdir(installRoot, { mode: 0o700 })
    const installed = resolve(await installer({ installRoot, manifest, platform, arch, target }))
    const boundary = `${resolve(installRoot)}${process.platform === 'win32' ? '\\' : '/'}`
    if (!installed.startsWith(boundary)) {
      throw new Error('Word PDF renderer installer escaped its private staging directory')
    }
    await rename(join(installed, 'libreoffice'), join(staging, 'libreoffice'))
    await rm(installRoot, { recursive: true, force: true })
    await probe(join(staging, ...target.executable.split('/')), manifest.libreOffice.version)
    const inspected = await inspectComponentTree(staging)
    const receipt = buildReceipt(manifest, target, platform, arch, inspected.files, inspected.links)
    await writeReceipt(staging, receipt)
    await verifyReceipt(staging, manifest, target, platform, arch)
    await hooks.afterStaging?.({ staging, receipt })
    await publishDirectoryAtomically(staging, outputDirectory, hooks)
    const published = await verifyReceipt(outputDirectory, manifest, target, platform, arch)
    return Object.freeze({ outputDirectory, receipt: published, reused: false })
  } finally {
    await rm(staging, { recursive: true, force: true }).catch(() => undefined)
  }
}

function parseArguments(argv) {
  const options = {}
  for (const argument of argv) {
    if (argument === '--verify') options.verifyOnly = true
    else if (argument === '--force') options.forceRebuild = true
    else if (argument === '--if-supported') options.ifSupported = true
    else throw new Error(`Unknown argument: ${argument}`)
  }
  return options
}

async function main() {
  const options = parseArguments(process.argv.slice(2))
  let result
  try {
    result = await prepareWordPdfRenderer(options)
  } catch (error) {
    if (options.ifSupported && /is not packaged for/.test(String(error?.message))) {
      console.log(`Skipped Word PDF renderer: ${error.message}`)
      return
    }
    throw error
  }
  console.log(
    `${options.verifyOnly ? 'Verified' : result.reused ? 'Reused' : 'Prepared'} Word PDF renderer ` +
      `${result.receipt.runtime.version} (${result.receipt.bundleRevision}) at ${result.outputDirectory}`
  )
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined
if (invokedPath === import.meta.url) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  })
}
