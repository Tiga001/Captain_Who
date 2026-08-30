/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { spawn } from 'node:child_process'
import { createHash, randomUUID } from 'node:crypto'
import { createReadStream, createWriteStream } from 'node:fs'
import { chmod, lstat, mkdir, open, readFile, readdir, rename, rm } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { basename, dirname, join, resolve } from 'node:path'
import { Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'

const RECEIPT_NAME = 'component-receipt.json'
const PROVIDER_ID = 'mycopilot.office-render-runtime'
const BUNDLE_VERSION = '2026.07.2'
const PLAYWRIGHT_VERSION = '1.61.1'
const BROWSER_VERSION = '149.0.7827.55'
const BROWSER_REVISION = '1228'
const BUNDLE_REVISION_PREFIX = 'office-render-runtime-sha256-v1:'
const MAX_FILES = 256
const MAX_TOTAL_BYTES = 1024 * 1024 * 1024
const MAX_FILE_BYTES = 512 * 1024 * 1024
const MAX_ARCHIVE_BYTES = 256 * 1024 * 1024
const MAX_RECEIPT_BYTES = 1024 * 1024
const SHA256_PATTERN = /^[a-f0-9]{64}$/
const SUPPORTED_PLATFORMS = new Set(['darwin', 'linux', 'win32'])
const SUPPORTED_ARCHITECTURES = new Set(['arm64', 'x64'])
const SCRIPT_DIRECTORY = dirname(fileURLToPath(import.meta.url))
const REPOSITORY_ROOT = resolve(SCRIPT_DIRECTORY, '..')
const DEFAULT_MANIFEST_PATH = join(REPOSITORY_ROOT, 'resources', 'office-renderer-manifest.json')
const DEFAULT_OUTPUT_DIRECTORY = join(REPOSITORY_ROOT, '.cache', 'office-renderer', 'current')
const localRequire = createRequire(import.meta.url)

function plainObject(value, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be an object`)
  }
  return value
}

function exactKeys(value, expectedKeys, label) {
  const actual = Object.keys(value).sort()
  const expected = [...expectedKeys].sort()
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    throw new Error(`${label} must contain exactly: ${expected.join(', ')}`)
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
    throw new Error(`${label} must be a canonical relative path`)
  }
  return path
}

function pinnedArchive(value, label) {
  const archive = plainObject(value, label)
  exactKeys(archive, ['url', 'size', 'sha256'], label)
  const url = new URL(nonEmptyString(archive.url, `${label}.url`))
  if (
    url.protocol !== 'https:' ||
    url.hostname !== 'cdn.playwright.dev' ||
    url.username ||
    url.password ||
    url.port ||
    url.search ||
    url.hash
  ) {
    throw new Error(`${label}.url must be an exact HTTPS URL on cdn.playwright.dev`)
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

export function computeBundleRevision(receiptFields) {
  const value = plainObject(receiptFields, 'receipt fields')
  const payload = Object.fromEntries(
    Object.entries(value).filter(([key]) => key !== 'bundleRevision')
  )
  const digest = createHash('sha256')
    .update(JSON.stringify(canonicalValue(payload)))
    .digest('hex')
  return `${BUNDLE_REVISION_PREFIX}${digest}`
}

export function validateOfficeRendererManifest(value) {
  const manifest = plainObject(value, 'manifest')
  exactKeys(
    manifest,
    ['schemaVersion', 'providerId', 'bundleVersion', 'browser', 'targets'],
    'manifest'
  )
  if (manifest.schemaVersion !== 2) throw new Error('manifest.schemaVersion must be 2')
  if (manifest.providerId !== PROVIDER_ID) {
    throw new Error(`manifest.providerId must be ${PROVIDER_ID}`)
  }
  if (manifest.bundleVersion !== BUNDLE_VERSION) {
    throw new Error(`Office renderer bundle must remain pinned to ${BUNDLE_VERSION}`)
  }

  const browser = plainObject(manifest.browser, 'manifest.browser')
  exactKeys(browser, ['family', 'version', 'playwrightVersion', 'revision'], 'manifest.browser')
  if (
    browser.family !== 'chromium' ||
    browser.version !== BROWSER_VERSION ||
    browser.playwrightVersion !== PLAYWRIGHT_VERSION ||
    browser.revision !== BROWSER_REVISION
  ) {
    throw new Error(
      `Office renderer must pin Chromium ${BROWSER_VERSION}, Playwright ${PLAYWRIGHT_VERSION}, revision ${BROWSER_REVISION}`
    )
  }

  const targets = plainObject(manifest.targets, 'manifest.targets')
  const expectedTargets = []
  for (const platform of SUPPORTED_PLATFORMS) {
    for (const arch of SUPPORTED_ARCHITECTURES) expectedTargets.push(`${platform}-${arch}`)
  }
  exactKeys(targets, expectedTargets, 'manifest.targets')
  const validatedTargets = Object.fromEntries(
    expectedTargets.map((key) => {
      const target = plainObject(targets[key], `manifest.targets.${key}`)
      exactKeys(target, ['executable', 'archive'], `manifest.targets.${key}`)
      const executable = canonicalRelativePath(
        target.executable,
        `manifest.targets.${key}.executable`
      )
      if (!executable.startsWith('browser/')) {
        throw new Error(`manifest.targets.${key}.executable must be inside browser/`)
      }
      const archive = pinnedArchive(target.archive, `manifest.targets.${key}.archive`)
      return [key, Object.freeze({ executable, archive })]
    })
  )

  return Object.freeze({
    schemaVersion: 2,
    providerId: PROVIDER_ID,
    bundleVersion: BUNDLE_VERSION,
    browser: Object.freeze({ ...browser }),
    targets: Object.freeze(validatedTargets)
  })
}

export async function loadOfficeRendererManifest(manifestPath = DEFAULT_MANIFEST_PATH) {
  let parsed
  try {
    parsed = JSON.parse(await readFile(manifestPath, 'utf8'))
  } catch (error) {
    throw new Error(`Office renderer manifest is not valid JSON: ${error.message}`, {
      cause: error
    })
  }
  return validateOfficeRendererManifest(parsed)
}

export function selectOfficeRendererTarget(
  manifest,
  platform = process.platform,
  arch = process.arch
) {
  if (!SUPPORTED_PLATFORMS.has(platform) || !SUPPORTED_ARCHITECTURES.has(arch)) {
    throw new Error(`Office renderer is not packaged for ${platform}-${arch}`)
  }
  const target = manifest.targets[`${platform}-${arch}`]
  if (!target) throw new Error(`Office renderer manifest has no target for ${platform}-${arch}`)
  return target
}

async function hashFile(path) {
  const digest = createHash('sha256')
  for await (const chunk of createReadStream(path)) digest.update(chunk)
  return digest.digest('hex')
}

async function syncDirectory(path) {
  if (process.platform === 'win32') return
  const directory = await open(path, 'r')
  try {
    await directory.sync()
  } finally {
    await directory.close()
  }
}

async function syncRegularFile(path) {
  const file = await open(path, 'r')
  try {
    await file.sync()
  } finally {
    await file.close()
  }
}

async function inspectComponentTree(root) {
  const rootMetadata = await lstat(root)
  if (!rootMetadata.isDirectory() || rootMetadata.isSymbolicLink()) {
    throw new Error('Office renderer component root must be a real, non-symlink directory')
  }
  const files = []
  const directories = []
  let totalBytes = 0
  const pending = [{ directory: root, prefix: '' }]
  while (pending.length > 0) {
    const current = pending.pop()
    const entries = await readdir(current.directory, { withFileTypes: true })
    for (const entry of entries) {
      const path = join(current.directory, entry.name)
      const logical = current.prefix ? `${current.prefix}/${entry.name}` : entry.name
      const metadata = await lstat(path)
      if (metadata.isSymbolicLink()) {
        throw new Error(`Office renderer component cannot contain symlink ${logical}`)
      }
      if (metadata.isDirectory()) {
        directories.push(logical)
        pending.push({ directory: path, prefix: logical })
        continue
      }
      if (!metadata.isFile()) {
        throw new Error(`Office renderer entry is not a regular file: ${logical}`)
      }
      if (logical === RECEIPT_NAME) continue
      if (files.length >= MAX_FILES) throw new Error('Office renderer exceeds its file-count limit')
      if (metadata.size > MAX_FILE_BYTES) {
        throw new Error(`Office renderer file exceeds its byte limit: ${logical}`)
      }
      totalBytes += metadata.size
      if (totalBytes > MAX_TOTAL_BYTES) {
        throw new Error('Office renderer exceeds its total byte limit')
      }
      files.push({ path: logical, size: metadata.size, sha256: await hashFile(path) })
    }
  }
  const comparePath = (left, right) => Buffer.from(left).compare(Buffer.from(right))
  files.sort((left, right) => comparePath(left.path, right.path))
  directories.sort(comparePath)
  return { files, directories }
}

function directoriesForFiles(files) {
  const directories = new Set()
  for (const file of files) {
    let parent = dirname(file.path).replaceAll('\\', '/')
    while (parent !== '.') {
      directories.add(parent)
      parent = dirname(parent).replaceAll('\\', '/')
    }
  }
  return [...directories].sort((left, right) => Buffer.from(left).compare(Buffer.from(right)))
}

async function syncTree(root) {
  const pending = [root]
  const directories = []
  while (pending.length > 0) {
    const directory = pending.pop()
    directories.push(directory)
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      const metadata = await lstat(path)
      if (metadata.isSymbolicLink()) {
        throw new Error(`Office renderer staging contains a forbidden symlink: ${path}`)
      }
      if (metadata.isDirectory()) pending.push(path)
      else if (metadata.isFile()) await syncRegularFile(path)
      else throw new Error(`Office renderer staging contains a special file: ${path}`)
    }
  }
  directories.sort((left, right) => right.length - left.length)
  for (const directory of directories) await syncDirectory(directory)
}

function readPlaywrightPin() {
  // Office rendering remains pinned to its independently reviewed browser runtime. The
  // application-level `playwright` dependency follows the managed Playwright MCP version.
  const packageJsonPath = localRequire.resolve('@mycopilot/office-playwright-runtime/package.json')
  const playwrightRoot = dirname(packageJsonPath)
  const playwrightCoreRoot = resolve(playwrightRoot, '..', 'playwright-core')
  return Promise.all([
    readFile(packageJsonPath, 'utf8'),
    readFile(resolve(playwrightCoreRoot, 'browsers.json'), 'utf8')
  ]).then(([packageJsonSource, browsersSource]) => {
    const packageJson = JSON.parse(packageJsonSource)
    const browsers = JSON.parse(browsersSource)
    const shell = browsers.browsers?.find(({ name }) => name === 'chromium-headless-shell')
    if (
      packageJson.version !== PLAYWRIGHT_VERSION ||
      shell?.revision !== BROWSER_REVISION ||
      shell?.browserVersion !== BROWSER_VERSION
    ) {
      throw new Error('Installed Playwright package does not match the pinned Office renderer')
    }
    const coreBundle = localRequire(join(playwrightCoreRoot, 'lib', 'coreBundle.js'))
    if (
      typeof coreBundle.utils?.extractZip !== 'function' ||
      typeof coreBundle.utils?.httpRequest !== 'function'
    ) {
      throw new Error('Pinned Playwright package does not expose its trusted archive helpers')
    }
    return {
      extractZip: coreBundle.utils.extractZip,
      httpRequest: coreBundle.utils.httpRequest
    }
  })
}

export async function downloadPinnedArchive(archive, archivePath, httpRequest) {
  const response = await new Promise((resolvePromise, rejectPromise) => {
    httpRequest({ url: archive.url, socketTimeout: 60_000 }, resolvePromise, rejectPromise)
  })
  if (response.statusCode !== 200) {
    response.resume()
    throw new Error(`Pinned Office renderer archive returned HTTP ${response.statusCode ?? 0}`)
  }
  const contentLength = Number(response.headers['content-length'])
  if (!Number.isSafeInteger(contentLength) || contentLength !== archive.size) {
    response.destroy()
    throw new Error('Pinned Office renderer archive Content-Length does not match the manifest')
  }

  const digest = createHash('sha256')
  let bytes = 0
  const verifier = new Transform({
    transform(chunk, _encoding, callback) {
      bytes += chunk.length
      if (bytes > archive.size || bytes > MAX_ARCHIVE_BYTES) {
        callback(new Error('Pinned Office renderer archive exceeds its frozen byte limit'))
        return
      }
      digest.update(chunk)
      callback(null, chunk)
    }
  })
  await pipeline(response, verifier, createWriteStream(archivePath, { flags: 'wx', mode: 0o600 }))
  if (bytes !== archive.size || digest.digest('hex') !== archive.sha256) {
    throw new Error('Pinned Office renderer archive failed its frozen SHA-256 identity check')
  }
  await syncRegularFile(archivePath)
}

async function runProcess(executable, args, { env, timeoutMs = 600_000 } = {}) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(executable, args, {
      env: env ?? process.env,
      shell: false,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true
    })
    const stdout = []
    const stderr = []
    let stdoutBytes = 0
    let stderrBytes = 0
    let settled = false
    const finish = (callback) => {
      if (settled) return false
      settled = true
      clearTimeout(timer)
      callback()
      return true
    }
    child.stdout.on('data', (chunk) => {
      stdoutBytes += chunk.length
      if (stdoutBytes <= 2 * 1024 * 1024) stdout.push(chunk)
    })
    child.stderr.on('data', (chunk) => {
      stderrBytes += chunk.length
      if (stderrBytes <= 2 * 1024 * 1024) stderr.push(chunk)
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
        else {
          rejectPromise(
            new Error(
              `${basename(executable)} failed with ${code ?? signal}: ${result.stderr.trim()}`
            )
          )
        }
      })
    )
    const timer = setTimeout(() => {
      child.kill('SIGKILL')
      finish(() => rejectPromise(new Error(`${basename(executable)} timed out`)))
    }, timeoutMs)
  })
}

export async function installPinnedOfficeRenderer({ installRoot, platform, arch, target }) {
  if (platform !== process.platform || arch !== process.arch) {
    throw new Error(
      `Playwright can only prepare the current host target (${process.platform}-${process.arch})`
    )
  }
  const { extractZip, httpRequest } = await readPlaywrightPin()
  const archivePath = join(installRoot, 'chromium-headless-shell.zip')
  const installed = join(installRoot, 'extracted')
  await mkdir(installed, { recursive: false, mode: 0o700 })
  try {
    // The archive's trusted size and SHA-256 are checked before extraction and
    // before any downloaded executable is started.
    await downloadPinnedArchive(target.archive, archivePath, httpRequest)
    await extractZip(archivePath, { dir: installed })
  } finally {
    await rm(archivePath, { force: true })
  }
  return installed
}

async function probeExecutable(path, expectedVersion) {
  const metadata = await lstat(path)
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error('Office renderer executable must be a regular non-symlink file')
  }
  if (process.platform !== 'win32') await chmod(path, 0o755)
  const result = await runProcess(path, ['--version'], { timeoutMs: 15_000 })
  const reported = `${result.stdout}\n${result.stderr}`.trim()
  if (!reported.includes(expectedVersion)) {
    throw new Error(`Office renderer reported an unexpected browser version: ${reported}`)
  }
}

function buildReceipt(manifest, target, platform, arch, files) {
  const payload = {
    schemaVersion: 2,
    providerId: manifest.providerId,
    bundleVersion: manifest.bundleVersion,
    platform,
    arch,
    browser: {
      family: manifest.browser.family,
      version: manifest.browser.version,
      playwrightVersion: manifest.browser.playwrightVersion,
      revision: manifest.browser.revision,
      executable: target.executable
    },
    archive: target.archive,
    files
  }
  return { ...payload, bundleRevision: computeBundleRevision(payload) }
}

async function writeReceipt(staging, receipt) {
  const path = join(staging, RECEIPT_NAME)
  const handle = await open(path, 'wx', 0o600)
  try {
    await handle.writeFile(`${JSON.stringify(receipt, null, 2)}\n`)
    await handle.sync()
  } finally {
    await handle.close()
  }
  if (process.platform !== 'win32') await chmod(path, 0o644)
  await syncDirectory(staging)
}

async function replaceReceiptAtomically(outputDirectory, receipt) {
  const receiptPath = join(outputDirectory, RECEIPT_NAME)
  const temporaryPath = join(outputDirectory, `.${RECEIPT_NAME}.${process.pid}.${randomUUID()}.tmp`)
  const bytes = Buffer.from(`${JSON.stringify(receipt, null, 2)}\n`, 'utf8')
  if (bytes.length <= 0 || bytes.length > MAX_RECEIPT_BYTES) {
    throw new Error('Office renderer replacement receipt is empty or oversized')
  }
  const handle = await open(temporaryPath, 'wx', 0o600)
  try {
    await handle.writeFile(bytes)
    await handle.sync()
  } finally {
    await handle.close()
  }
  try {
    if (process.platform !== 'win32') await chmod(temporaryPath, 0o644)
    await rename(temporaryPath, receiptPath)
    await syncDirectory(outputDirectory)
  } finally {
    await rm(temporaryPath, { force: true }).catch(() => undefined)
  }
}

function validateReceipt(receipt, manifest, target, platform, arch) {
  const value = plainObject(receipt, 'Office renderer receipt')
  exactKeys(
    value,
    [
      'schemaVersion',
      'providerId',
      'bundleVersion',
      'platform',
      'arch',
      'browser',
      'archive',
      'files',
      'bundleRevision'
    ],
    'Office renderer receipt'
  )
  if (
    value.schemaVersion !== 2 ||
    value.providerId !== manifest.providerId ||
    value.bundleVersion !== manifest.bundleVersion ||
    value.platform !== platform ||
    value.arch !== arch
  ) {
    throw new Error('Office renderer receipt identity does not match this build target')
  }
  exactKeys(
    plainObject(value.browser, 'Office renderer receipt.browser'),
    ['family', 'version', 'playwrightVersion', 'revision', 'executable'],
    'Office renderer receipt.browser'
  )
  const expectedBrowser = {
    family: manifest.browser.family,
    version: manifest.browser.version,
    playwrightVersion: manifest.browser.playwrightVersion,
    revision: manifest.browser.revision,
    executable: target.executable
  }
  if (JSON.stringify(value.browser) !== JSON.stringify(expectedBrowser)) {
    throw new Error('Office renderer receipt browser identity does not match the pinned manifest')
  }
  exactKeys(
    plainObject(value.archive, 'Office renderer receipt.archive'),
    ['url', 'size', 'sha256'],
    'Office renderer receipt.archive'
  )
  if (JSON.stringify(value.archive) !== JSON.stringify(target.archive)) {
    throw new Error('Office renderer receipt archive identity does not match the pinned manifest')
  }
  if (!Array.isArray(value.files) || value.files.length === 0 || value.files.length > MAX_FILES) {
    throw new Error('Office renderer receipt files are invalid')
  }
  let priorPath
  let totalBytes = 0
  for (const [index, file] of value.files.entries()) {
    const descriptor = plainObject(file, `Office renderer receipt.files[${index}]`)
    exactKeys(descriptor, ['path', 'size', 'sha256'], `Office renderer receipt.files[${index}]`)
    const path = canonicalRelativePath(
      descriptor.path,
      `Office renderer receipt.files[${index}].path`
    )
    if (!path.startsWith('browser/'))
      throw new Error('Office renderer files must be inside browser/')
    if (priorPath !== undefined && Buffer.compare(Buffer.from(priorPath), Buffer.from(path)) >= 0) {
      throw new Error('Office renderer receipt files must be uniquely sorted by path')
    }
    priorPath = path
    if (!Number.isSafeInteger(descriptor.size) || descriptor.size < 0) {
      throw new Error('Office renderer receipt contains an invalid file size')
    }
    if (descriptor.size > MAX_FILE_BYTES) {
      throw new Error('Office renderer receipt contains an oversized file')
    }
    totalBytes += descriptor.size
    if (totalBytes > MAX_TOTAL_BYTES) throw new Error('Office renderer receipt exceeds byte limit')
    if (!SHA256_PATTERN.test(descriptor.sha256)) {
      throw new Error('Office renderer receipt contains an invalid SHA-256 digest')
    }
  }
  if (!value.files.some(({ path }) => path === target.executable)) {
    throw new Error('Office renderer receipt does not contain its executable')
  }
  if (value.bundleRevision !== computeBundleRevision(value)) {
    throw new Error('Office renderer bundle revision does not match its receipt')
  }
  return value
}

async function verifyReceipt(outputDirectory, manifest, target, platform, arch) {
  const receiptPath = join(outputDirectory, RECEIPT_NAME)
  const receiptMetadata = await lstat(receiptPath)
  if (!receiptMetadata.isFile() || receiptMetadata.isSymbolicLink()) {
    throw new Error('Office renderer receipt must be a regular non-symlink file')
  }
  if (receiptMetadata.size <= 0 || receiptMetadata.size > MAX_RECEIPT_BYTES) {
    throw new Error('Office renderer receipt is empty or oversized')
  }
  const bytes = await readFile(receiptPath)
  if (bytes.includes(0)) throw new Error('Office renderer receipt contains a NUL byte')
  const receipt = validateReceipt(
    JSON.parse(bytes.toString('utf8')),
    manifest,
    target,
    platform,
    arch
  )
  const executablePath = join(outputDirectory, ...target.executable.split('/'))
  const executableMetadata = await lstat(executablePath)
  if (!executableMetadata.isFile() || executableMetadata.isSymbolicLink()) {
    throw new Error('Office renderer executable must be a regular non-symlink file')
  }
  if (process.platform !== 'win32' && (executableMetadata.mode & 0o111) === 0) {
    throw new Error('Office renderer executable must have a Unix execute bit')
  }
  const actual = await inspectComponentTree(outputDirectory)
  if (JSON.stringify(actual.files) !== JSON.stringify(receipt.files)) {
    throw new Error('Office renderer component files do not match the frozen receipt')
  }
  if (JSON.stringify(actual.directories) !== JSON.stringify(directoriesForFiles(receipt.files))) {
    throw new Error('Office renderer component contains unexpected or empty directories')
  }
  return receipt
}

export async function refreshOfficeRendererReceiptAfterSigning({
  outputDirectory,
  originalReceipt,
  signedPaths,
  manifestPath = DEFAULT_MANIFEST_PATH,
  platform = process.platform,
  arch = process.arch
}) {
  if (!Array.isArray(signedPaths) || signedPaths.length === 0) {
    throw new Error('Office renderer signing must declare at least one mutated path')
  }
  const canonicalSignedPaths = signedPaths.map((path, index) =>
    canonicalRelativePath(path, `Office renderer signedPaths[${index}]`)
  )
  const signedPathSet = new Set(canonicalSignedPaths)
  if (signedPathSet.size !== canonicalSignedPaths.length) {
    throw new Error('Office renderer signing paths must be unique')
  }

  const manifest = await loadOfficeRendererManifest(manifestPath)
  const target = selectOfficeRendererTarget(manifest, platform, arch)
  const frozen = validateReceipt(originalReceipt, manifest, target, platform, arch)
  const receiptPath = join(outputDirectory, RECEIPT_NAME)
  const currentReceiptMetadata = await lstat(receiptPath)
  if (!currentReceiptMetadata.isFile() || currentReceiptMetadata.isSymbolicLink()) {
    throw new Error('Office renderer receipt must remain a regular non-symlink file during signing')
  }
  const currentReceipt = JSON.parse(await readFile(receiptPath, 'utf8'))
  if (JSON.stringify(currentReceipt) !== JSON.stringify(frozen)) {
    throw new Error('Office renderer receipt changed during the signing transaction')
  }

  const frozenByPath = new Map(frozen.files.map((file) => [file.path, file]))
  for (const path of signedPathSet) {
    if (!frozenByPath.has(path)) {
      throw new Error(`Office renderer signing path is absent from the frozen receipt: ${path}`)
    }
  }

  const actual = await inspectComponentTree(outputDirectory)
  if (JSON.stringify(actual.directories) !== JSON.stringify(directoriesForFiles(frozen.files))) {
    throw new Error('Office renderer directories changed during the signing transaction')
  }
  if (
    actual.files.length !== frozen.files.length ||
    actual.files.some((file, index) => file.path !== frozen.files[index].path)
  ) {
    throw new Error('Office renderer file set changed during the signing transaction')
  }

  const changedPaths = new Set()
  for (const actualFile of actual.files) {
    const frozenFile = frozenByPath.get(actualFile.path)
    if (actualFile.size !== frozenFile.size || actualFile.sha256 !== frozenFile.sha256) {
      changedPaths.add(actualFile.path)
    }
  }
  for (const changedPath of changedPaths) {
    if (!signedPathSet.has(changedPath)) {
      throw new Error(`Office renderer file outside the signing allowlist changed: ${changedPath}`)
    }
  }
  for (const signedPath of signedPathSet) {
    if (!changedPaths.has(signedPath)) {
      throw new Error(`Office renderer signing did not change expected Mach-O: ${signedPath}`)
    }
  }

  const payload = { ...frozen, files: actual.files }
  const refreshed = { ...payload, bundleRevision: computeBundleRevision(payload) }
  validateReceipt(refreshed, manifest, target, platform, arch)
  await replaceReceiptAtomically(outputDirectory, refreshed)
  return verifyReceipt(outputDirectory, manifest, target, platform, arch)
}

async function publishDirectoryAtomically(staging, outputDirectory, hooks) {
  const parent = dirname(outputDirectory)
  const backup = join(parent, `.${basename(outputDirectory)}.${process.pid}.${randomUUID()}.backup`)
  let previousMoved = false
  try {
    await hooks.beforePublish?.({ staging, outputDirectory })
    try {
      await rename(outputDirectory, backup)
      previousMoved = true
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
    await rename(staging, outputDirectory)
    await syncDirectory(parent)
  } catch (error) {
    if (previousMoved) {
      await rename(backup, outputDirectory).catch(() => undefined)
      await syncDirectory(parent).catch(() => undefined)
    }
    throw error
  }
  if (previousMoved) {
    await rm(backup, { recursive: true, force: true })
    await syncDirectory(parent)
  }
}

export async function prepareOfficeRenderer({
  manifestPath = DEFAULT_MANIFEST_PATH,
  outputDirectory = DEFAULT_OUTPUT_DIRECTORY,
  platform = process.platform,
  arch = process.arch,
  verifyOnly = false,
  forceRebuild = false,
  installer = installPinnedOfficeRenderer,
  hooks = {}
} = {}) {
  const manifest = await loadOfficeRendererManifest(manifestPath)
  const target = selectOfficeRendererTarget(manifest, platform, arch)
  await readPlaywrightPin()
  if (verifyOnly) {
    const receipt = await verifyReceipt(outputDirectory, manifest, target, platform, arch)
    return Object.freeze({ outputDirectory, receipt, reused: true })
  }
  if (!forceRebuild) {
    try {
      const receipt = await verifyReceipt(outputDirectory, manifest, target, platform, arch)
      return Object.freeze({ outputDirectory, receipt, reused: true })
    } catch {
      // Missing, stale, or damaged components are rebuilt from the frozen, hashed archive.
    }
  }

  const parent = dirname(outputDirectory)
  await mkdir(parent, { recursive: true })
  const staging = join(
    parent,
    `.${basename(outputDirectory)}.${process.pid}.${randomUUID()}.staging`
  )
  await mkdir(staging, { recursive: false, mode: 0o700 })
  try {
    const installRoot = join(staging, '.playwright-install')
    await mkdir(installRoot, { recursive: false, mode: 0o700 })
    const installedRoot = resolve(
      await installer({ installRoot, manifest, platform, arch, target })
    )
    const installBoundary = `${resolve(installRoot)}${process.platform === 'win32' ? '\\' : '/'}`
    if (!installedRoot.startsWith(installBoundary)) {
      throw new Error('Office renderer installer returned a path outside its private staging root')
    }
    const browserRoot = join(staging, 'browser')
    await rename(installedRoot, browserRoot)
    await rm(installRoot, { recursive: true, force: true })
    const inspected = await inspectComponentTree(staging)
    if (
      JSON.stringify(inspected.directories) !== JSON.stringify(directoriesForFiles(inspected.files))
    ) {
      throw new Error('Office renderer staging contains unexpected or empty directories')
    }
    // Inspect the authenticated archive's extracted tree before its executable
    // receives control. This rejects links, special files and resource bombs.
    await probeExecutable(join(staging, ...target.executable.split('/')), manifest.browser.version)
    await syncTree(staging)
    const receipt = buildReceipt(manifest, target, platform, arch, inspected.files)
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
    else throw new Error(`Unknown argument: ${argument}`)
  }
  return options
}

async function main() {
  const options = parseArguments(process.argv.slice(2))
  const result = await prepareOfficeRenderer(options)
  console.log(
    `${options.verifyOnly ? 'Verified' : result.reused ? 'Reused' : 'Prepared'} Office renderer ` +
      `${result.receipt.browser.version} (${result.receipt.bundleRevision}) at ${result.outputDirectory}`
  )
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined
if (invokedPath === import.meta.url) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  })
}
