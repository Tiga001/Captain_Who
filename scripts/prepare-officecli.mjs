/* eslint-disable @typescript-eslint/explicit-function-return-type -- JavaScript packaging script; runtime validation defines its public contract. */

import { createHash, randomUUID } from 'node:crypto'
import { createReadStream } from 'node:fs'
import { chmod, lstat, mkdir, open, readFile, readdir, rename, unlink } from 'node:fs/promises'
import { get as httpsGet } from 'node:https'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  assertExactMachOSigningPaths,
  assertOnlyFrozenMachOFilesChanged,
  collectFrozenMachOTargets
} from './frozen-macho-signing.mjs'

export const OFFICECLI_MAX_DOWNLOAD_BYTES = 64 * 1024 * 1024
export const OFFICECLI_MAX_REDIRECTS = 5

const ALLOWED_DOWNLOAD_HOSTS = new Set([
  'github.com',
  'raw.githubusercontent.com',
  'release-assets.githubusercontent.com',
  'objects.githubusercontent.com',
  'github-releases.githubusercontent.com'
])
const SUPPORTED_PLATFORMS = new Set(['darwin', 'linux', 'win32'])
const SUPPORTED_ARCHITECTURES = new Set(['arm64', 'x64'])
const SHA256_PATTERN = /^[a-f0-9]{64}$/
const COMPONENT_RECEIPT_NAME = 'component-receipt.json'
const OFFICECLI_BUNDLE_REVISION_PREFIX = 'officecli-bundle-sha256-v1:'
const OFFICECLI_MAX_RECEIPT_BYTES = 1024 * 1024
const SCRIPT_DIRECTORY = dirname(fileURLToPath(import.meta.url))
const REPOSITORY_ROOT = resolve(SCRIPT_DIRECTORY, '..')
const DEFAULT_MANIFEST_PATH = join(REPOSITORY_ROOT, 'resources', 'officecli-manifest.json')
const DEFAULT_OUTPUT_DIRECTORY = join(REPOSITORY_ROOT, '.cache', 'officecli', 'current')
const GENERATED_TEMP_FILE_PATTERN =
  /^\.(?:officecli(?:\.exe)?|LICENSE|NOTICE|THIRD-PARTY-NOTICES\.txt|component-receipt\.json)\.\d+\.[0-9a-f-]{36}\.(?:download|tmp)$/

function requirePlainObject(value, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be an object`)
  }
  return value
}

function requireExactKeys(value, expectedKeys, label) {
  const actual = Object.keys(value).sort()
  const expected = [...expectedKeys].sort()
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    throw new Error(`${label} must contain exactly: ${expected.join(', ')}`)
  }
}

function requireNonEmptyString(value, label) {
  if (typeof value !== 'string' || value.trim() !== value || value.length === 0) {
    throw new Error(`${label} must be a non-empty, trimmed string`)
  }
  return value
}

function requirePositiveSafeInteger(value, label) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`${label} must be a positive safe integer`)
  }
  return value
}

function validateSha256(value, label) {
  const digest = requireNonEmptyString(value, label)
  if (!SHA256_PATTERN.test(digest)) {
    throw new Error(`${label} must be a lowercase SHA-256 digest`)
  }
  return digest
}

function validateFileName(value, label) {
  const name = requireNonEmptyString(value, label)
  if (name === '.' || name === '..' || name.includes('/') || name.includes('\\')) {
    throw new Error(`${label} must be a single file name`)
  }
  return name
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

export function computeOfficeCliBundleRevision(receiptFields) {
  const value = requirePlainObject(receiptFields, 'OfficeCLI receipt fields')
  const payload = Object.fromEntries(
    Object.entries(value).filter(([key]) => key !== 'bundleRevision')
  )
  const digest = createHash('sha256')
    .update(JSON.stringify(canonicalValue(payload)))
    .digest('hex')
  return `${OFFICECLI_BUNDLE_REVISION_PREFIX}${digest}`
}

export function validateDownloadUrl(value, label = 'download URL') {
  const rawUrl = requireNonEmptyString(value, label)
  let url
  try {
    url = new URL(rawUrl)
  } catch {
    throw new Error(`${label} is not a valid URL`)
  }

  if (url.protocol !== 'https:') {
    throw new Error(`${label} must use HTTPS`)
  }
  if (url.username || url.password) {
    throw new Error(`${label} must not contain credentials`)
  }
  if (url.port) {
    throw new Error(`${label} must not use a custom port`)
  }
  if (!ALLOWED_DOWNLOAD_HOSTS.has(url.hostname.toLowerCase())) {
    throw new Error(`${label} host is not allowlisted: ${url.hostname}`)
  }
  return url
}

function validateDownloadDescriptor(value, label, maxDownloadBytes) {
  const descriptor = requirePlainObject(value, label)
  const size = requirePositiveSafeInteger(descriptor.size, `${label}.size`)
  if (size > maxDownloadBytes || size > OFFICECLI_MAX_DOWNLOAD_BYTES) {
    throw new Error(`${label}.size exceeds the 64 MiB component limit`)
  }

  return Object.freeze({
    sourceName: validateFileName(descriptor.sourceName, `${label}.sourceName`),
    targetName: validateFileName(descriptor.targetName, `${label}.targetName`),
    url: validateDownloadUrl(descriptor.url, `${label}.url`).href,
    size,
    sha256: validateSha256(descriptor.sha256, `${label}.sha256`)
  })
}

export function validateManifest(value) {
  const manifest = requirePlainObject(value, 'manifest')
  if (manifest.schemaVersion !== 1) {
    throw new Error('manifest.schemaVersion must be 1')
  }

  const component = requirePlainObject(manifest.component, 'manifest.component')
  const maxDownloadBytes = requirePositiveSafeInteger(
    component.maxDownloadBytes,
    'manifest.component.maxDownloadBytes'
  )
  if (maxDownloadBytes !== OFFICECLI_MAX_DOWNLOAD_BYTES) {
    throw new Error('manifest.component.maxDownloadBytes must be exactly 64 MiB')
  }

  const validatedComponent = Object.freeze({
    id: requireNonEmptyString(component.id, 'manifest.component.id'),
    name: requireNonEmptyString(component.name, 'manifest.component.name'),
    version: requireNonEmptyString(component.version, 'manifest.component.version'),
    source: validateDownloadUrl(component.source, 'manifest.component.source').href,
    license: requireNonEmptyString(component.license, 'manifest.component.license'),
    maxDownloadBytes
  })
  if (validatedComponent.id !== 'officecli') {
    throw new Error('manifest.component.id must be officecli')
  }
  if (validatedComponent.version !== '1.0.139') {
    throw new Error('OfficeCLI component must remain pinned to version 1.0.139')
  }

  const assets = requirePlainObject(manifest.assets, 'manifest.assets')
  const validatedAssets = {}
  const expectedKeys = []
  for (const platform of SUPPORTED_PLATFORMS) {
    for (const arch of SUPPORTED_ARCHITECTURES) {
      expectedKeys.push(`${platform}-${arch}`)
    }
  }
  if (
    Object.keys(assets).length !== expectedKeys.length ||
    expectedKeys.some((key) => !Object.hasOwn(assets, key))
  ) {
    throw new Error(`manifest.assets must contain exactly: ${expectedKeys.sort().join(', ')}`)
  }

  for (const key of expectedKeys) {
    const asset = requirePlainObject(assets[key], `manifest.assets.${key}`)
    const expectedTargetName = asset.platform === 'win32' ? 'officecli.exe' : 'officecli'
    const descriptor = validateDownloadDescriptor(asset, `manifest.assets.${key}`, maxDownloadBytes)
    if (`${asset.platform}-${asset.arch}` !== key) {
      throw new Error(`manifest.assets.${key} platform/architecture does not match its key`)
    }
    if (!SUPPORTED_PLATFORMS.has(asset.platform) || !SUPPORTED_ARCHITECTURES.has(asset.arch)) {
      throw new Error(`manifest.assets.${key} uses an unsupported platform or architecture`)
    }
    if (descriptor.targetName !== expectedTargetName) {
      throw new Error(`manifest.assets.${key}.targetName must be ${expectedTargetName}`)
    }
    validatedAssets[key] = Object.freeze({
      ...descriptor,
      platform: asset.platform,
      arch: asset.arch
    })
  }

  if (!Array.isArray(manifest.legalFiles) || manifest.legalFiles.length !== 3) {
    throw new Error('manifest.legalFiles must contain LICENSE, NOTICE, and THIRD-PARTY-NOTICES.txt')
  }
  const validatedLegalFiles = manifest.legalFiles.map((entry, index) =>
    validateDownloadDescriptor(entry, `manifest.legalFiles[${index}]`, maxDownloadBytes)
  )
  const expectedLegalNames = new Set(['LICENSE', 'NOTICE', 'THIRD-PARTY-NOTICES.txt'])
  if (
    validatedLegalFiles.some((file) => !expectedLegalNames.delete(file.targetName)) ||
    expectedLegalNames.size !== 0
  ) {
    throw new Error('manifest.legalFiles must target LICENSE, NOTICE, and THIRD-PARTY-NOTICES.txt')
  }

  return Object.freeze({
    schemaVersion: 1,
    component: validatedComponent,
    assets: Object.freeze(validatedAssets),
    legalFiles: Object.freeze(validatedLegalFiles)
  })
}

export async function loadAndValidateManifest(manifestPath = DEFAULT_MANIFEST_PATH) {
  const source = await readFile(manifestPath, 'utf8')
  let value
  try {
    value = JSON.parse(source)
  } catch (error) {
    throw new Error(`OfficeCLI manifest is not valid JSON: ${error.message}`, { cause: error })
  }
  return validateManifest(value)
}

export function selectAsset(manifest, platform = process.platform, arch = process.arch) {
  if (!SUPPORTED_PLATFORMS.has(platform) || !SUPPORTED_ARCHITECTURES.has(arch)) {
    throw new Error(
      `OfficeCLI v${manifest.component.version} is not packaged for ${platform}-${arch}; ` +
        'supported targets are darwin/linux/win32 on arm64 or x64'
    )
  }
  const asset = manifest.assets[`${platform}-${arch}`]
  if (!asset) {
    throw new Error(`OfficeCLI manifest has no asset for ${platform}-${arch}`)
  }
  return asset
}

async function sha256File(filePath) {
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(filePath)) {
    hash.update(chunk)
  }
  return hash.digest('hex')
}

async function fileMatches(filePath, descriptor) {
  try {
    const metadata = await lstat(filePath)
    if (!metadata.isFile() || metadata.size !== descriptor.size) {
      return false
    }
    return (await sha256File(filePath)) === descriptor.sha256
  } catch (error) {
    if (error?.code === 'ENOENT') {
      return false
    }
    throw error
  }
}

async function requestDownload(url, redirectsRemaining = OFFICECLI_MAX_REDIRECTS) {
  validateDownloadUrl(url)
  return new Promise((resolvePromise, rejectPromise) => {
    const request = httpsGet(
      url,
      {
        headers: {
          Accept: 'application/octet-stream',
          'User-Agent': 'CaptainWho-OfficeCLI-Component-Preparer/1'
        }
      },
      (response) => {
        const statusCode = response.statusCode ?? 0
        const location = response.headers.location
        if ([301, 302, 303, 307, 308].includes(statusCode)) {
          response.resume()
          if (!location) {
            rejectPromise(
              new Error(`OfficeCLI download returned redirect ${statusCode} without Location`)
            )
            return
          }
          if (redirectsRemaining === 0) {
            rejectPromise(
              new Error(`OfficeCLI download exceeded ${OFFICECLI_MAX_REDIRECTS} redirects`)
            )
            return
          }

          let redirectUrl
          try {
            redirectUrl = new URL(location, url)
            validateDownloadUrl(redirectUrl.href, 'redirect URL')
          } catch (error) {
            rejectPromise(error)
            return
          }

          requestDownload(redirectUrl.href, redirectsRemaining - 1).then(
            resolvePromise,
            rejectPromise
          )
          return
        }

        if (statusCode !== 200) {
          response.resume()
          rejectPromise(new Error(`OfficeCLI download failed with HTTP ${statusCode}`))
          return
        }
        resolvePromise(response)
      }
    )
    request.setTimeout(30_000, () => request.destroy(new Error('OfficeCLI download timed out')))
    request.on('error', rejectPromise)
  })
}

async function syncDirectory(directoryPath) {
  if (process.platform === 'win32') {
    return
  }
  const directory = await open(directoryPath, 'r')
  try {
    await directory.sync()
  } finally {
    await directory.close()
  }
}

async function unlinkIfPresent(filePath) {
  try {
    await unlink(filePath)
  } catch (error) {
    if (error?.code !== 'ENOENT') {
      throw error
    }
  }
}

async function sanitizeOutputDirectory(outputDirectory, executableName) {
  const expectedNames = new Set([
    executableName,
    'LICENSE',
    'NOTICE',
    'THIRD-PARTY-NOTICES.txt',
    COMPONENT_RECEIPT_NAME
  ])
  const staleExecutableName = executableName === 'officecli.exe' ? 'officecli' : 'officecli.exe'
  let changed = false

  for (const entry of await readdir(outputDirectory, { withFileTypes: true })) {
    if (entry.name === staleExecutableName || GENERATED_TEMP_FILE_PATTERN.test(entry.name)) {
      if (entry.isDirectory()) {
        throw new Error(`Unexpected directory in OfficeCLI component cache: ${entry.name}`)
      }
      await unlink(join(outputDirectory, entry.name))
      changed = true
      continue
    }
    if (!expectedNames.has(entry.name) || !entry.isFile()) {
      throw new Error(`Unexpected entry in OfficeCLI component cache: ${entry.name}`)
    }
  }

  if (changed) {
    await syncDirectory(outputDirectory)
  }
}

async function publishBufferAtomically(destinationPath, bytes) {
  const directoryPath = dirname(destinationPath)
  await mkdir(directoryPath, { recursive: true })
  const temporaryPath = join(
    directoryPath,
    `.${destinationPath.split(/[\\/]/).at(-1)}.${process.pid}.${randomUUID()}.tmp`
  )
  let handle
  try {
    handle = await open(temporaryPath, 'wx', 0o600)
    await handle.writeFile(bytes)
    await handle.sync()
    await handle.close()
    handle = undefined
    await rename(temporaryPath, destinationPath)
    await syncDirectory(directoryPath)
  } finally {
    await handle?.close().catch(() => undefined)
    await unlinkIfPresent(temporaryPath)
  }
}

async function downloadAndPublish(descriptor, destinationPath, maxDownloadBytes) {
  if (await fileMatches(destinationPath, descriptor)) {
    return false
  }

  const directoryPath = dirname(destinationPath)
  await mkdir(directoryPath, { recursive: true })
  const temporaryPath = join(
    directoryPath,
    `.${descriptor.targetName}.${process.pid}.${randomUUID()}.download`
  )
  let handle
  try {
    const response = await requestDownload(descriptor.url)
    const contentLengthHeader = response.headers['content-length']
    if (contentLengthHeader !== undefined) {
      const contentLength = Number(contentLengthHeader)
      if (!Number.isSafeInteger(contentLength) || contentLength < 0) {
        response.destroy()
        throw new Error(`Invalid Content-Length for ${descriptor.sourceName}`)
      }
      if (contentLength > maxDownloadBytes || contentLength > OFFICECLI_MAX_DOWNLOAD_BYTES) {
        response.destroy()
        throw new Error(`${descriptor.sourceName} exceeds the 64 MiB component limit`)
      }
      if (contentLength !== descriptor.size) {
        response.destroy()
        throw new Error(
          `${descriptor.sourceName} Content-Length ${contentLength} does not match pinned size ${descriptor.size}`
        )
      }
    }

    handle = await open(temporaryPath, 'wx', 0o600)
    const hash = createHash('sha256')
    let receivedBytes = 0
    for await (const chunk of response) {
      receivedBytes += chunk.length
      if (receivedBytes > maxDownloadBytes || receivedBytes > OFFICECLI_MAX_DOWNLOAD_BYTES) {
        response.destroy()
        throw new Error(`${descriptor.sourceName} exceeds the 64 MiB component limit`)
      }
      hash.update(chunk)
      await handle.write(chunk)
    }

    const digest = hash.digest('hex')
    if (receivedBytes !== descriptor.size) {
      throw new Error(
        `${descriptor.sourceName} size ${receivedBytes} does not match pinned size ${descriptor.size}`
      )
    }
    if (digest !== descriptor.sha256) {
      throw new Error(`${descriptor.sourceName} failed SHA-256 verification`)
    }

    await handle.sync()
    await handle.close()
    handle = undefined
    await rename(temporaryPath, destinationPath)
    await syncDirectory(directoryPath)
    return true
  } finally {
    await handle?.close().catch(() => undefined)
    await unlinkIfPresent(temporaryPath)
  }
}

function buildExpectedFiles(manifest, asset) {
  return [asset, ...manifest.legalFiles]
}

function fileDescriptors(files) {
  return files.map((file) => ({
    name: file.targetName,
    size: file.size,
    sha256: file.sha256
  }))
}

function buildReceipt(
  manifest,
  asset,
  currentFiles = fileDescriptors(buildExpectedFiles(manifest, asset)),
  signedMachO = []
) {
  const payload = {
    schemaVersion: 2,
    component: manifest.component.id,
    version: manifest.component.version,
    source: manifest.component.source,
    license: manifest.component.license,
    platform: asset.platform,
    arch: asset.arch,
    executable: asset.targetName,
    sourceFiles: fileDescriptors(buildExpectedFiles(manifest, asset)),
    files: currentFiles.map(({ name, size, sha256 }) => ({ name, size, sha256 })),
    signedMachO: [...signedMachO]
  }
  return { ...payload, bundleRevision: computeOfficeCliBundleRevision(payload) }
}

async function verifyPreparedFiles(outputDirectory, files) {
  const failures = []
  for (const file of files) {
    if (!(await fileMatches(join(outputDirectory, file.targetName), file))) {
      failures.push(file.targetName)
    }
  }
  return failures
}

function validateReceiptFiles(value, expectedNames, label) {
  if (!Array.isArray(value) || value.length !== expectedNames.length) {
    throw new Error(`${label} must describe exactly ${expectedNames.length} files`)
  }
  let totalBytes = 0
  return value.map((entry, index) => {
    const descriptor = requirePlainObject(entry, `${label}[${index}]`)
    requireExactKeys(descriptor, ['name', 'size', 'sha256'], `${label}[${index}]`)
    const name = validateFileName(descriptor.name, `${label}[${index}].name`)
    if (name !== expectedNames[index]) {
      throw new Error(`${label} must preserve the frozen file order and names`)
    }
    if (!Number.isSafeInteger(descriptor.size) || descriptor.size < 0) {
      throw new Error(`${label}[${index}].size must be a non-negative safe integer`)
    }
    totalBytes += descriptor.size
    if (totalBytes > OFFICECLI_MAX_DOWNLOAD_BYTES * expectedNames.length) {
      throw new Error(`${label} exceeds the bounded component size`)
    }
    return Object.freeze({
      name,
      size: descriptor.size,
      sha256: validateSha256(descriptor.sha256, `${label}[${index}].sha256`)
    })
  })
}

function validateOfficeCliReceipt(receipt, manifest, asset, { signed }) {
  const value = requirePlainObject(receipt, 'OfficeCLI receipt')
  requireExactKeys(
    value,
    [
      'schemaVersion',
      'component',
      'version',
      'source',
      'license',
      'platform',
      'arch',
      'executable',
      'sourceFiles',
      'files',
      'signedMachO',
      'bundleRevision'
    ],
    'OfficeCLI receipt'
  )
  if (
    value.schemaVersion !== 2 ||
    value.component !== manifest.component.id ||
    value.version !== manifest.component.version ||
    value.source !== manifest.component.source ||
    value.license !== manifest.component.license ||
    value.platform !== asset.platform ||
    value.arch !== asset.arch ||
    value.executable !== asset.targetName
  ) {
    throw new Error('OfficeCLI receipt identity does not match the pinned build target')
  }

  const sourceFiles = fileDescriptors(buildExpectedFiles(manifest, asset))
  const expectedNames = sourceFiles.map(({ name }) => name)
  const validatedSourceFiles = validateReceiptFiles(
    value.sourceFiles,
    expectedNames,
    'OfficeCLI receipt.sourceFiles'
  )
  if (JSON.stringify(validatedSourceFiles) !== JSON.stringify(sourceFiles)) {
    throw new Error('OfficeCLI receipt source provenance does not match the pinned manifest')
  }
  const files = validateReceiptFiles(value.files, expectedNames, 'OfficeCLI receipt.files')
  if (!Array.isArray(value.signedMachO)) {
    throw new Error('OfficeCLI receipt.signedMachO must be an array')
  }

  if (signed) {
    if (JSON.stringify(value.signedMachO) !== JSON.stringify([asset.targetName])) {
      throw new Error('Signed OfficeCLI receipt must declare its one frozen Mach-O executable')
    }
    if (JSON.stringify(files[0]) === JSON.stringify(sourceFiles[0])) {
      throw new Error('Signed OfficeCLI receipt executable must differ from its upstream source')
    }
    if (JSON.stringify(files.slice(1)) !== JSON.stringify(sourceFiles.slice(1))) {
      throw new Error('Signed OfficeCLI receipt changed non-executable source files')
    }
  } else {
    if (value.signedMachO.length !== 0) {
      throw new Error('Unsigned OfficeCLI receipt cannot declare signed Mach-O files')
    }
    if (JSON.stringify(files) !== JSON.stringify(sourceFiles)) {
      throw new Error('Unsigned OfficeCLI receipt files do not match the pinned source files')
    }
  }
  if (value.bundleRevision !== computeOfficeCliBundleRevision(value)) {
    throw new Error('OfficeCLI bundle revision does not match its receipt')
  }
  return value
}

async function inspectOfficeCliTree(outputDirectory, expectedFiles) {
  const rootMetadata = await lstat(outputDirectory)
  if (!rootMetadata.isDirectory() || rootMetadata.isSymbolicLink()) {
    throw new Error('OfficeCLI component root must be a real, non-symlink directory')
  }
  const expectedNames = new Set([
    ...expectedFiles.map(({ targetName }) => targetName),
    COMPONENT_RECEIPT_NAME
  ])
  const entries = await readdir(outputDirectory, { withFileTypes: true })
  if (
    entries.length !== expectedNames.size ||
    entries.some((entry) => !expectedNames.has(entry.name))
  ) {
    throw new Error('OfficeCLI component file set does not match its frozen receipt boundary')
  }
  for (const entry of entries) {
    const metadata = await lstat(join(outputDirectory, entry.name))
    if (!entry.isFile() || !metadata.isFile() || metadata.isSymbolicLink()) {
      throw new Error(`OfficeCLI component entry must be a regular non-symlink file: ${entry.name}`)
    }
  }

  const files = []
  for (const descriptor of expectedFiles) {
    const path = join(outputDirectory, descriptor.targetName)
    const metadata = await lstat(path)
    files.push({
      name: descriptor.targetName,
      size: metadata.size,
      sha256: await sha256File(path)
    })
  }
  return files
}

async function verifyOfficeCliReceipt(outputDirectory, manifest, asset, { signed }) {
  const receiptPath = join(outputDirectory, COMPONENT_RECEIPT_NAME)
  const receiptMetadata = await lstat(receiptPath)
  if (!receiptMetadata.isFile() || receiptMetadata.isSymbolicLink()) {
    throw new Error('OfficeCLI component receipt must be a regular non-symlink file')
  }
  if (receiptMetadata.size <= 0 || receiptMetadata.size > OFFICECLI_MAX_RECEIPT_BYTES) {
    throw new Error('OfficeCLI component receipt is empty or oversized')
  }
  const receiptBytes = await readFile(receiptPath)
  if (receiptBytes.includes(0)) {
    throw new Error('OfficeCLI component receipt contains a NUL byte')
  }
  let parsedReceipt
  try {
    parsedReceipt = JSON.parse(receiptBytes.toString('utf8'))
  } catch (error) {
    throw new Error(`OfficeCLI component receipt is not valid JSON: ${error.message}`, {
      cause: error
    })
  }
  const receipt = validateOfficeCliReceipt(parsedReceipt, manifest, asset, { signed })
  const expectedFiles = buildExpectedFiles(manifest, asset)
  const actualFiles = await inspectOfficeCliTree(outputDirectory, expectedFiles)
  if (JSON.stringify(actualFiles) !== JSON.stringify(receipt.files)) {
    throw new Error('OfficeCLI component files do not match the frozen receipt')
  }
  if (asset.platform !== 'win32') {
    const executableMetadata = await lstat(join(outputDirectory, asset.targetName))
    if ((executableMetadata.mode & 0o111) === 0) {
      throw new Error('OfficeCLI executable must have a Unix execute bit')
    }
  }
  return receipt
}

export async function prepareOfficeCli({
  manifestPath = DEFAULT_MANIFEST_PATH,
  outputDirectory = DEFAULT_OUTPUT_DIRECTORY,
  platform = process.platform,
  arch = process.arch,
  verifyOnly = false
} = {}) {
  const manifest = await loadAndValidateManifest(manifestPath)
  const asset = selectAsset(manifest, platform, arch)
  const files = buildExpectedFiles(manifest, asset)
  const receiptPath = join(outputDirectory, COMPONENT_RECEIPT_NAME)

  if (verifyOnly) {
    const receipt = await verifyOfficeCliReceipt(outputDirectory, manifest, asset, {
      signed: false
    })
    return Object.freeze({
      version: manifest.component.version,
      platform,
      arch,
      executablePath: join(outputDirectory, asset.targetName),
      outputDirectory,
      receipt
    })
  }

  await mkdir(outputDirectory, { recursive: true })
  await sanitizeOutputDirectory(outputDirectory, asset.targetName)
  for (const file of files) {
    await downloadAndPublish(
      file,
      join(outputDirectory, file.targetName),
      manifest.component.maxDownloadBytes
    )
  }

  const failures = await verifyPreparedFiles(outputDirectory, files)
  if (failures.length > 0) {
    throw new Error(`OfficeCLI component verification failed: ${failures.join(', ')}`)
  }

  if (platform !== 'win32') {
    await chmod(join(outputDirectory, asset.targetName), 0o755)
    for (const legalFile of manifest.legalFiles) {
      await chmod(join(outputDirectory, legalFile.targetName), 0o644)
    }
  }
  const receipt = buildReceipt(manifest, asset)
  const receiptBytes = Buffer.from(`${JSON.stringify(receipt, null, 2)}\n`, 'utf8')
  const currentReceipt = await readFile(receiptPath).catch((error) => {
    if (error?.code === 'ENOENT') {
      return undefined
    }
    throw error
  })
  if (!currentReceipt?.equals(receiptBytes)) {
    await publishBufferAtomically(receiptPath, receiptBytes)
  }
  if (platform !== 'win32') {
    await chmod(receiptPath, 0o644)
  }

  const verifiedReceipt = await verifyOfficeCliReceipt(outputDirectory, manifest, asset, {
    signed: false
  })
  return Object.freeze({
    version: manifest.component.version,
    platform,
    arch,
    executablePath: join(outputDirectory, asset.targetName),
    outputDirectory,
    receipt: verifiedReceipt
  })
}

export async function prepareOfficeCliMacSigning({
  manifestPath = DEFAULT_MANIFEST_PATH,
  outputDirectory = DEFAULT_OUTPUT_DIRECTORY,
  platform = 'darwin',
  arch = process.arch
} = {}) {
  if (platform !== 'darwin') {
    throw new Error(`OfficeCLI macOS signing requires darwin, got ${platform}`)
  }
  const prepared = await prepareOfficeCli({
    manifestPath,
    outputDirectory,
    platform,
    arch,
    verifyOnly: true
  })
  const targets = await collectFrozenMachOTargets({
    outputDirectory,
    files: prepared.receipt.files,
    pathKey: 'name',
    label: 'OfficeCLI'
  })
  if (targets.length !== 1 || targets[0].relativePath !== prepared.receipt.executable) {
    throw new Error('OfficeCLI frozen component must contain exactly its one Mach-O executable')
  }
  return Object.freeze({ outputDirectory, receipt: prepared.receipt, targets })
}

export async function refreshOfficeCliReceiptAfterSigning({
  outputDirectory,
  originalReceipt,
  signedPaths,
  manifestPath = DEFAULT_MANIFEST_PATH,
  platform = 'darwin',
  arch = process.arch
}) {
  if (platform !== 'darwin') {
    throw new Error(`OfficeCLI macOS signing requires darwin, got ${platform}`)
  }
  const manifest = await loadAndValidateManifest(manifestPath)
  const asset = selectAsset(manifest, platform, arch)
  const frozen = validateOfficeCliReceipt(originalReceipt, manifest, asset, { signed: false })
  const receiptPath = join(outputDirectory, COMPONENT_RECEIPT_NAME)
  const receiptMetadata = await lstat(receiptPath)
  if (!receiptMetadata.isFile() || receiptMetadata.isSymbolicLink()) {
    throw new Error('OfficeCLI receipt must remain a regular non-symlink file during signing')
  }
  const currentReceipt = JSON.parse(await readFile(receiptPath, 'utf8'))
  if (JSON.stringify(currentReceipt) !== JSON.stringify(frozen)) {
    throw new Error('OfficeCLI receipt changed during the signing transaction')
  }

  const actualFiles = await inspectOfficeCliTree(
    outputDirectory,
    buildExpectedFiles(manifest, asset)
  )
  const targets = await collectFrozenMachOTargets({
    outputDirectory,
    files: actualFiles,
    pathKey: 'name',
    label: 'OfficeCLI'
  })
  if (targets.length !== 1 || targets[0].relativePath !== asset.targetName) {
    throw new Error('OfficeCLI Mach-O file set changed during the signing transaction')
  }
  const canonicalSignedPaths = assertExactMachOSigningPaths({
    signedPaths,
    targets,
    label: 'OfficeCLI'
  })
  assertOnlyFrozenMachOFilesChanged({
    beforeFiles: frozen.files,
    afterFiles: actualFiles,
    signedPaths: canonicalSignedPaths,
    pathKey: 'name',
    label: 'OfficeCLI'
  })

  const refreshed = buildReceipt(manifest, asset, actualFiles, canonicalSignedPaths)
  validateOfficeCliReceipt(refreshed, manifest, asset, { signed: true })
  await publishBufferAtomically(
    receiptPath,
    Buffer.from(`${JSON.stringify(refreshed, null, 2)}\n`, 'utf8')
  )
  if (process.platform !== 'win32') await chmod(receiptPath, 0o644)
  return verifyOfficeCliReceipt(outputDirectory, manifest, asset, { signed: true })
}

export async function verifyPackagedOfficeCli({
  manifestPath = DEFAULT_MANIFEST_PATH,
  outputDirectory,
  platform = 'darwin',
  arch = process.arch,
  signed = true
}) {
  if (typeof outputDirectory !== 'string' || outputDirectory.length === 0) {
    throw new Error('Packaged OfficeCLI output directory is required')
  }
  const manifest = await loadAndValidateManifest(manifestPath)
  const asset = selectAsset(manifest, platform, arch)
  const receipt = await verifyOfficeCliReceipt(outputDirectory, manifest, asset, { signed })
  return Object.freeze({ outputDirectory, receipt })
}

function parseArguments(argv) {
  const unsupported = argv.filter((argument) => argument !== '--verify')
  if (unsupported.length > 0) {
    throw new Error(`Unknown argument(s): ${unsupported.join(', ')}`)
  }
  return { verifyOnly: argv.includes('--verify') }
}

async function main() {
  const options = parseArguments(process.argv.slice(2))
  const prepared = await prepareOfficeCli(options)
  console.log(
    `${options.verifyOnly ? 'Verified' : 'Prepared'} OfficeCLI v${prepared.version} for ` +
      `${prepared.platform}-${prepared.arch}: ${prepared.executablePath}`
  )
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined
if (invokedPath === import.meta.url) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  })
}
