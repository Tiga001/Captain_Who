/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { createHash, randomUUID } from 'node:crypto'
import { spawn } from 'node:child_process'
import { constants as fsConstants } from 'node:fs'
import { createRequire, isBuiltin } from 'node:module'
import { createReadStream } from 'node:fs'
import {
  chmod,
  cp,
  lstat,
  mkdir,
  open,
  readFile,
  readdir,
  realpath,
  rename,
  rm,
  unlink,
  writeFile
} from 'node:fs/promises'
import { get as httpsGet } from 'node:https'
import { basename, dirname, isAbsolute, join, relative, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { extract } from 'tar'

export const ARTIFACT_RUNTIME_MAX_DOWNLOAD_BYTES = 128 * 1024 * 1024
export const ARTIFACT_RUNTIME_MAX_REDIRECTS = 5
export const ARTIFACT_RUNTIME_MAX_FILES = 100_000
export const ARTIFACT_RUNTIME_MAX_TOTAL_BYTES = 4 * 1024 * 1024 * 1024

const RECEIPT_NAME = 'component-receipt.json'
const BUNDLE_REVISION_PREFIX = 'artifact-runtime-bundle-sha256-v1:'
const BUILD_INPUTS_REVISION_PREFIX = 'artifact-runtime-build-inputs-sha256-v1:'
const SUPPORTED_PLATFORMS = new Set(['darwin', 'linux', 'win32'])
const SUPPORTED_ARCHITECTURES = new Set(['arm64', 'x64'])
const ALLOWED_DOWNLOAD_HOSTS = new Set([
  'github.com',
  'raw.githubusercontent.com',
  'release-assets.githubusercontent.com',
  'objects.githubusercontent.com',
  'nodejs.org',
  'files.pythonhosted.org',
  'pypi.org'
])
const SHA256_PATTERN = /^[a-f0-9]{64}$/
const SAFE_ENVIRONMENT = Object.freeze({
  PIP_DISABLE_PIP_VERSION_CHECK: '1',
  PIP_NO_INPUT: '1',
  PYTHONDONTWRITEBYTECODE: '1',
  PYTHONNOUSERSITE: '1',
  PYTHONUTF8: '1'
})
const SCRIPT_DIRECTORY = dirname(fileURLToPath(import.meta.url))
const REPOSITORY_ROOT = resolve(SCRIPT_DIRECTORY, '..')
const DEFAULT_MANIFEST_PATH = join(REPOSITORY_ROOT, 'resources', 'artifact-runtime-manifest.json')
const DEFAULT_OUTPUT_DIRECTORY = join(REPOSITORY_ROOT, '.cache', 'artifact-runtime', 'current')
const DEFAULT_DOWNLOAD_DIRECTORY = join(REPOSITORY_ROOT, '.cache', 'artifact-runtime', 'downloads')
const NODE_WORKSPACE_PACKAGE = join(REPOSITORY_ROOT, 'packages', 'artifact-runtime-node')
const NODE_PACKAGE_EVIDENCE_SOURCE = join(
  REPOSITORY_ROOT,
  'resources',
  'artifact-runtime-node-package-evidence.json'
)
const NODE_PACKAGE_EVIDENCE_TARGET = 'dependencies/node/node-package-evidence.json'
const NODE_BOOTSTRAP_SOURCE = join(
  REPOSITORY_ROOT,
  'resources',
  'artifact-runtime',
  'node-bootstrap.mjs'
)
const NODE_LOADER_SOURCE = join(REPOSITORY_ROOT, 'resources', 'artifact-runtime', 'node-loader.mjs')
const BUILD_INPUT_RELATIVE_PATHS = Object.freeze({
  builder: 'scripts/prepare-artifact-runtime.mjs',
  nodeBootstrap: 'resources/artifact-runtime/node-bootstrap.mjs',
  nodeLoader: 'resources/artifact-runtime/node-loader.mjs',
  nodePackageManifest: 'packages/artifact-runtime-node/package.json',
  nodePackageEvidence: 'resources/artifact-runtime-node-package-evidence.json',
  pnpmLockfile: 'pnpm-lock.yaml',
  pythonRequirements: 'resources/artifact-runtime-python-requirements.txt'
})
const BUILD_INPUT_SOURCE_PATHS = Object.freeze(
  Object.fromEntries(
    Object.entries(BUILD_INPUT_RELATIVE_PATHS).map(([id, path]) => [
      id,
      join(REPOSITORY_ROOT, ...path.split('/'))
    ])
  )
)

// These packages are exact, reviewed exceptions because their published archive does not contain
// a conventional LICENSE/COPYING/NOTICE file. A new version, changed declaration, or missing
// evidence file fails closed. `buffers@0.1.1` is intentionally NOASSERTION: its npm archive has no
// license declaration at all, and the exception is made visible in the shipped audit inventory
// instead of inventing a license expression.
const NODE_METADATA_LICENSE_EVIDENCE_ALLOWLIST = Object.freeze({
  'binary@0.3.0': Object.freeze({
    declaredLicense: 'MIT',
    licenseExpression: 'MIT',
    evidenceFiles: Object.freeze(['README.markdown']),
    review: 'package metadata and the shipped README declare MIT'
  }),
  'buffers@0.1.1': Object.freeze({
    declaredLicense: null,
    licenseExpression: 'NOASSERTION',
    evidenceFiles: Object.freeze(['package.json']),
    review: 'exact reviewed exception: the published archive declares no license'
  }),
  'chainsaw@0.1.0': Object.freeze({
    declaredLicense: 'MIT/X11',
    licenseExpression: 'MIT',
    evidenceFiles: Object.freeze(['package.json']),
    review: 'package metadata uses the legacy MIT/X11 identifier'
  }),
  'hash.js@1.1.7': Object.freeze({
    declaredLicense: 'MIT',
    licenseExpression: 'MIT',
    evidenceFiles: Object.freeze(['README.md']),
    review: 'package metadata and the shipped README contain the MIT grant'
  }),
  'isarray@1.0.0': Object.freeze({
    declaredLicense: 'MIT',
    licenseExpression: 'MIT',
    evidenceFiles: Object.freeze(['README.md']),
    review: 'package metadata and the shipped README contain the MIT grant'
  }),
  'saxes@5.0.1': Object.freeze({
    declaredLicense: 'ISC',
    licenseExpression: 'ISC',
    evidenceFiles: Object.freeze(['package.json']),
    review: 'package metadata declares ISC'
  })
})

const PYTHON_METADATA_LICENSE_EVIDENCE_ALLOWLIST = Object.freeze({
  'et-xmlfile@2.0.0': Object.freeze({
    declaredLicense: 'MIT',
    licenseExpression: 'MIT',
    review: 'wheel METADATA declares MIT and its OSI-approved MIT classifier'
  }),
  'openpyxl@3.1.5': Object.freeze({
    declaredLicense: 'MIT',
    licenseExpression: 'MIT',
    review: 'wheel METADATA declares MIT and its OSI-approved MIT classifier'
  }),
  'pdfplumber@0.11.9': Object.freeze({
    declaredLicense: null,
    licenseExpression: 'MIT',
    review: 'wheel ships the upstream MIT LICENSE.txt while METADATA omits a license field'
  }),
  'pypdfium2@5.12.1': Object.freeze({
    declaredLicense: 'BSD-3-Clause, Apache-2.0, dependency licenses',
    licenseExpression: 'NOASSERTION',
    review:
      'wheel ships Apache-2.0, BSD-3-Clause, CC-BY-4.0, PDFium, and build-dependency license evidence without one aggregate SPDX expression'
  }),
  'reportlab@4.4.9': Object.freeze({
    declaredLicense:
      'BSD license (see license.txt for details), Copyright (c) 2000-2025, ReportLab Inc.',
    licenseExpression: 'BSD-3-Clause',
    review: 'wheel ships the ReportLab BSD license and METADATA identifies it as BSD'
  })
})

const LICENSE_FILE_PATTERN = /^(licen[cs]e|copying|notice|copyright)([._-].*)?$/i
const MAX_LEGAL_EVIDENCE_FILES_PER_PACKAGE = 64
const MAX_LEGAL_EVIDENCE_FILE_BYTES = 2 * 1024 * 1024
const MAX_NODE_PACKAGE_EVIDENCE_BYTES = 16 * 1024 * 1024
const MAX_NODE_PACKAGE_FILES = 50_000
const MAX_NODE_PACKAGE_BYTES = 1024 * 1024 * 1024

function plainObject(value, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be an object`)
  }
  return value
}

function exactKeys(value, keys, label) {
  const actual = Object.keys(value).sort()
  const expected = [...keys].sort()
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

function positiveInteger(value, label) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`${label} must be a positive safe integer`)
  }
  return value
}

function sha256(value, label) {
  const digest = nonEmptyString(value, label)
  if (!SHA256_PATTERN.test(digest)) {
    throw new Error(`${label} must be a lowercase SHA-256 digest`)
  }
  return digest
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

export function validateArtifactRuntimeDownloadUrl(value, label = 'download URL') {
  const raw = nonEmptyString(value, label)
  let url
  try {
    url = new URL(raw)
  } catch {
    throw new Error(`${label} is not a valid URL`)
  }
  if (url.protocol !== 'https:') {
    throw new Error(`${label} must use HTTPS`)
  }
  if (url.username || url.password || url.port) {
    throw new Error(`${label} cannot contain credentials or a custom port`)
  }
  if (!ALLOWED_DOWNLOAD_HOSTS.has(url.hostname.toLowerCase())) {
    throw new Error(`${label} host is not allowlisted: ${url.hostname}`)
  }
  return url
}

function validateAsset(value, label, { node = false } = {}) {
  const asset = plainObject(value, label)
  const allowedKeys = node
    ? asset.format === 'tar.gz'
      ? ['format', 'archiveRoot', 'url', 'size', 'sha256']
      : ['format', 'url', 'size', 'sha256']
    : ['url', 'size', 'sha256']
  exactKeys(asset, allowedKeys, label)
  if (node && !['tar.gz', 'executable'].includes(asset.format)) {
    throw new Error(`${label}.format must be tar.gz or executable`)
  }
  const size = positiveInteger(asset.size, `${label}.size`)
  if (size > ARTIFACT_RUNTIME_MAX_DOWNLOAD_BYTES) {
    throw new Error(`${label}.size exceeds the 128 MiB component download limit`)
  }
  return Object.freeze({
    ...(node ? { format: asset.format } : {}),
    ...(asset.archiveRoot
      ? { archiveRoot: canonicalRelativePath(asset.archiveRoot, `${label}.archiveRoot`) }
      : {}),
    url: validateArtifactRuntimeDownloadUrl(asset.url, `${label}.url`).href,
    size,
    sha256: sha256(asset.sha256, `${label}.sha256`)
  })
}

function validateExecutableMap(value, label) {
  const map = plainObject(value, label)
  exactKeys(map, ['unix', 'win32'], label)
  return Object.freeze({
    unix: canonicalRelativePath(map.unix, `${label}.unix`),
    win32: canonicalRelativePath(map.win32, `${label}.win32`)
  })
}

function validateDependencies(value, expected, label) {
  if (!Array.isArray(value) || value.length !== expected.length) {
    throw new Error(`${label} must contain exactly ${expected.length} pinned dependencies`)
  }
  const dependencies = value.map((entry, index) => {
    const dependency = plainObject(entry, `${label}[${index}]`)
    exactKeys(dependency, ['name', 'version', 'identityFile'], `${label}[${index}]`)
    return Object.freeze({
      name: nonEmptyString(dependency.name, `${label}[${index}].name`),
      version: nonEmptyString(dependency.version, `${label}[${index}].version`),
      identityFile: canonicalRelativePath(
        dependency.identityFile,
        `${label}[${index}].identityFile`
      )
    })
  })
  const actual = new Map(dependencies.map((entry) => [entry.name.toLowerCase(), entry.version]))
  for (const [name, version] of expected) {
    if (actual.get(name) !== version) {
      throw new Error(`${label} must pin ${name}@${version}`)
    }
  }
  return Object.freeze(dependencies)
}

function validateTargetAssets(value, label, options) {
  const assets = plainObject(value, label)
  const targets = []
  for (const platform of SUPPORTED_PLATFORMS) {
    for (const arch of SUPPORTED_ARCHITECTURES) {
      targets.push(`${platform}-${arch}`)
    }
  }
  exactKeys(assets, targets, label)
  return Object.freeze(
    Object.fromEntries(
      targets.map((target) => [
        target,
        validateAsset(assets[target], `${label}.${target}`, options)
      ])
    )
  )
}

function validateBuildInputs(value) {
  const inputs = plainObject(value, 'manifest.buildInputs')
  const ids = Object.keys(BUILD_INPUT_RELATIVE_PATHS)
  exactKeys(inputs, ids, 'manifest.buildInputs')
  return Object.freeze(
    Object.fromEntries(
      ids.map((id) => {
        const descriptor = plainObject(inputs[id], `manifest.buildInputs.${id}`)
        exactKeys(descriptor, ['path', 'sha256'], `manifest.buildInputs.${id}`)
        const path = canonicalRelativePath(descriptor.path, `manifest.buildInputs.${id}.path`)
        if (path !== BUILD_INPUT_RELATIVE_PATHS[id]) {
          throw new Error(
            `manifest.buildInputs.${id}.path must remain ${BUILD_INPUT_RELATIVE_PATHS[id]}`
          )
        }
        return [
          id,
          Object.freeze({
            path,
            sha256: sha256(descriptor.sha256, `manifest.buildInputs.${id}.sha256`)
          })
        ]
      })
    )
  )
}

export function validateArtifactRuntimeManifest(value) {
  const manifest = plainObject(value, 'manifest')
  exactKeys(
    manifest,
    ['schemaVersion', 'providerId', 'bundleVersion', 'buildInputs', 'node', 'python'],
    'manifest'
  )
  if (manifest.schemaVersion !== 3) {
    throw new Error('manifest.schemaVersion must be 3')
  }
  if (manifest.providerId !== 'mycopilot.artifact-runtime') {
    throw new Error('manifest.providerId must be mycopilot.artifact-runtime')
  }
  if (manifest.bundleVersion !== '2026.08.1') {
    throw new Error('artifact runtime bundle must remain pinned to 2026.08.1')
  }
  const buildInputs = validateBuildInputs(manifest.buildInputs)

  const node = plainObject(manifest.node, 'manifest.node')
  exactKeys(
    node,
    [
      'version',
      'executable',
      'packageRoot',
      'bootstrap',
      'loader',
      'assets',
      'dependencies',
      'licenseFile'
    ],
    'manifest.node'
  )
  if (node.version !== '22.23.1') {
    throw new Error('managed Node must remain pinned to 22.23.1')
  }
  const licenseFile = plainObject(node.licenseFile, 'manifest.node.licenseFile')
  exactKeys(licenseFile, ['target', 'url', 'size', 'sha256'], 'manifest.node.licenseFile')

  const python = plainObject(manifest.python, 'manifest.python')
  exactKeys(
    python,
    [
      'version',
      'release',
      'executable',
      'runtimeHome',
      'requirements',
      'dependencies',
      'assets',
      'source',
      'license'
    ],
    'manifest.python'
  )
  if (python.version !== '3.12.13' || python.release !== '20260610') {
    throw new Error('managed Python must remain pinned to 3.12.13+20260610')
  }

  return Object.freeze({
    schemaVersion: 3,
    providerId: manifest.providerId,
    bundleVersion: manifest.bundleVersion,
    buildInputs,
    node: Object.freeze({
      version: node.version,
      executable: validateExecutableMap(node.executable, 'manifest.node.executable'),
      packageRoot: canonicalRelativePath(node.packageRoot, 'manifest.node.packageRoot'),
      bootstrap: canonicalRelativePath(node.bootstrap, 'manifest.node.bootstrap'),
      loader: canonicalRelativePath(node.loader, 'manifest.node.loader'),
      assets: validateTargetAssets(node.assets, 'manifest.node.assets', { node: true }),
      dependencies: validateDependencies(
        node.dependencies,
        [
          ['docx', '9.6.1'],
          ['exceljs', '4.4.0'],
          ['pptxgenjs', '4.0.1']
        ],
        'manifest.node.dependencies'
      ),
      licenseFile: Object.freeze({
        target: canonicalRelativePath(licenseFile.target, 'manifest.node.licenseFile.target'),
        url: validateArtifactRuntimeDownloadUrl(licenseFile.url, 'manifest.node.licenseFile.url')
          .href,
        size: positiveInteger(licenseFile.size, 'manifest.node.licenseFile.size'),
        sha256: sha256(licenseFile.sha256, 'manifest.node.licenseFile.sha256')
      })
    }),
    python: Object.freeze({
      version: python.version,
      release: python.release,
      executable: validateExecutableMap(python.executable, 'manifest.python.executable'),
      runtimeHome: canonicalRelativePath(python.runtimeHome, 'manifest.python.runtimeHome'),
      requirements: canonicalRelativePath(python.requirements, 'manifest.python.requirements'),
      dependencies: validateDependencies(
        python.dependencies,
        [
          ['openpyxl', '3.1.5'],
          ['pdfplumber', '0.11.9'],
          ['pypdf', '6.15.0'],
          ['pypdfium2', '5.12.1'],
          ['python-docx', '1.2.0'],
          ['python-pptx', '1.0.2'],
          ['reportlab', '4.4.9'],
          ['xlsxwriter', '3.2.9']
        ],
        'manifest.python.dependencies'
      ),
      assets: validateTargetAssets(python.assets, 'manifest.python.assets'),
      source: validateArtifactRuntimeDownloadUrl(python.source, 'manifest.python.source').href,
      license: nonEmptyString(python.license, 'manifest.python.license')
    })
  })
}

export async function loadArtifactRuntimeManifest(manifestPath = DEFAULT_MANIFEST_PATH) {
  let parsed
  try {
    parsed = JSON.parse(await readFile(manifestPath, 'utf8'))
  } catch (error) {
    throw new Error(`Artifact runtime manifest is not valid JSON: ${error.message}`, {
      cause: error
    })
  }
  const manifest = validateArtifactRuntimeManifest(parsed)
  await verifyArtifactRuntimeBuildInputs(manifest)
  return manifest
}

export async function verifyArtifactRuntimeBuildInputs(manifest) {
  for (const [id, descriptor] of Object.entries(manifest.buildInputs)) {
    await verifyPinnedLocalFile(
      BUILD_INPUT_SOURCE_PATHS[id],
      descriptor.sha256,
      `Artifact Runtime build input ${id}`
    )
  }
}

export function selectArtifactRuntimeAssets(
  manifest,
  platform = process.platform,
  arch = process.arch
) {
  if (!SUPPORTED_PLATFORMS.has(platform) || !SUPPORTED_ARCHITECTURES.has(arch)) {
    throw new Error(`Managed Artifact Runtime is not packaged for ${platform}-${arch}`)
  }
  const target = `${platform}-${arch}`
  return Object.freeze({
    target,
    node: manifest.node.assets[target],
    python: manifest.python.assets[target]
  })
}

async function hashFile(path) {
  const digest = createHash('sha256')
  for await (const chunk of createReadStream(path)) {
    digest.update(chunk)
  }
  return digest.digest('hex')
}

async function verifyPinnedLocalFile(path, expectedSha256, label) {
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

async function fileMatches(path, descriptor) {
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

async function requestDownload(url, redirectsRemaining = ARTIFACT_RUNTIME_MAX_REDIRECTS) {
  validateArtifactRuntimeDownloadUrl(url)
  return new Promise((resolvePromise, rejectPromise) => {
    const request = httpsGet(
      url,
      {
        headers: {
          Accept: 'application/octet-stream',
          'User-Agent': 'MyCopilot-Artifact-Runtime-Preparer/1'
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

async function syncDirectory(path) {
  if (process.platform === 'win32') return
  const directory = await open(path, 'r')
  try {
    await directory.sync()
  } finally {
    await directory.close()
  }
}

async function unlinkIfPresent(path) {
  try {
    await unlink(path)
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }
}

async function downloadPinnedFile(descriptor, destination) {
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

function archiveCachePath(downloadDirectory, descriptor) {
  const url = new URL(descriptor.url)
  const name = basename(decodeURIComponent(url.pathname))
  return join(downloadDirectory, `${descriptor.sha256.slice(0, 16)}-${name}`)
}

async function runProcess(executable, args, options = {}) {
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

async function copyWithoutSymlinks(source, destination, { destinationExists = false } = {}) {
  await cp(source, destination, {
    recursive: true,
    dereference: true,
    errorOnExist: !destinationExists,
    force: destinationExists,
    preserveTimestamps: true
  })
}

async function copyTreeRejectingSymlinks(source, destination, { destinationExists = false } = {}) {
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

async function acquireNodeRuntime(manifest, asset, staging, downloadDirectory) {
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

function pathIsWithin(root, candidate) {
  const remainder = relative(root, candidate)
  return (
    remainder === '' ||
    (!isAbsolute(remainder) &&
      remainder !== '..' &&
      !remainder.startsWith('../') &&
      !remainder.startsWith('..\\'))
  )
}

async function canonicalDirectoryWithin(path, allowedRoot, label) {
  const metadata = await lstat(path)
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new Error(`${label} must be a real, non-symlink directory: ${path}`)
  }
  const canonical = await realpath(path)
  if (!pathIsWithin(allowedRoot, canonical)) {
    throw new Error(`${label} escapes the approved package source boundary: ${path}`)
  }
  return canonical
}

async function readRegularFileNoFollow(path, label) {
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

async function findPackageRoot(name, fromDirectory, allowedPackageRoot) {
  const resolver = createRequire(join(fromDirectory, 'package.json'))
  let entry
  try {
    entry = resolver.resolve(`${name}/package.json`)
  } catch {
    try {
      entry = resolver.resolve(name)
    } catch (error) {
      const missing = new Error(
        `Cannot resolve locked Node dependency ${name} from ${fromDirectory}`,
        {
          cause: error
        }
      )
      missing.code = 'MANAGED_NODE_DEPENDENCY_NOT_FOUND'
      throw missing
    }
  }
  let current = dirname(entry)
  while (true) {
    const canonical = await canonicalDirectoryWithin(
      current,
      allowedPackageRoot,
      `Node dependency ${name}`
    )
    const packageJson = join(current, 'package.json')
    try {
      const { bytes } = await readRegularFileNoFollow(
        packageJson,
        `Node dependency ${name} package metadata`
      )
      const document = JSON.parse(bytes.toString('utf8'))
      if (document.name === name) return { root: canonical, document }
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
    const parent = dirname(current)
    if (parent === current) break
    current = parent
  }
  throw new Error(`Cannot locate package root for ${name}`)
}

function unquotePnpmKey(value) {
  if (value.startsWith("'") && value.endsWith("'")) {
    return value.slice(1, -1).replaceAll("''", "'")
  }
  if (value.startsWith('"') && value.endsWith('"')) return JSON.parse(value)
  return value
}

export function parsePnpmLockPackageIntegrities(text) {
  if (typeof text !== 'string' || text.includes('\0')) {
    throw new Error('pnpm lockfile must be NUL-free UTF-8 text')
  }
  const integrities = new Map()
  let inPackages = false
  let packageKey = null
  for (const line of text.split(/\r?\n/)) {
    if (line === 'packages:') {
      inPackages = true
      continue
    }
    if (inPackages && /^\S/.test(line)) break
    if (!inPackages) continue
    const packageMatch = /^ {2}(\S.*):$/.exec(line)
    if (packageMatch) {
      packageKey = unquotePnpmKey(packageMatch[1])
      continue
    }
    const integrityMatch = /^ {4}resolution: \{[^}]*integrity: ([^, }]+)[^}]*\}$/.exec(line)
    if (packageKey && integrityMatch) {
      const integrity = integrityMatch[1]
      if (!/^sha(?:256|384|512)-[A-Za-z0-9+/]+={0,2}$/.test(integrity)) {
        throw new Error(`pnpm lock package ${packageKey} has an invalid integrity`)
      }
      integrities.set(packageKey, integrity)
    }
  }
  if (!inPackages || integrities.size === 0) {
    throw new Error('pnpm lockfile does not contain a package integrity table')
  }
  return integrities
}

function lockedIntegrityForPackage(integrities, name, version) {
  const identity = `${name}@${version}`
  const matches = new Set()
  for (const [key, integrity] of integrities) {
    if (key === identity || key.startsWith(`${identity}(`)) matches.add(integrity)
  }
  if (matches.size !== 1) {
    throw new Error(
      `pnpm lockfile must provide exactly one integrity for managed Node package ${identity}`
    )
  }
  return [...matches][0]
}

async function collectNodePackageFiles(packageRoot) {
  const pending = [{ directory: packageRoot, prefix: '' }]
  const files = []
  let totalBytes = 0
  while (pending.length > 0) {
    const { directory, prefix } = pending.pop()
    const entries = await readdir(directory, { withFileTypes: true })
    entries.sort((left, right) => left.name.localeCompare(right.name, 'en'))
    for (const entry of entries) {
      if (prefix === '' && entry.name === 'node_modules') continue
      const source = join(directory, entry.name)
      const logical = prefix ? `${prefix}/${entry.name}` : entry.name
      canonicalRelativePath(logical, 'managed Node package file path')
      const metadata = await lstat(source)
      if (metadata.isSymbolicLink()) {
        throw new Error(`Managed Node package contains a forbidden symlink: ${source}`)
      }
      if (metadata.isDirectory()) {
        pending.push({ directory: source, prefix: logical })
        continue
      }
      if (!metadata.isFile()) {
        throw new Error(`Managed Node package contains a non-regular entry: ${source}`)
      }
      if (files.length === MAX_NODE_PACKAGE_FILES) {
        throw new Error('Managed Node package graph exceeds the file-count limit')
      }
      totalBytes += metadata.size
      if (totalBytes > MAX_NODE_PACKAGE_BYTES) {
        throw new Error('Managed Node package graph exceeds the byte limit')
      }
      const { bytes, metadata: opened } = await readRegularFileNoFollow(
        source,
        'Managed Node package content'
      )
      files.push({
        path: logical,
        size: opened.size,
        sha256: createHash('sha256').update(bytes).digest('hex'),
        executable: (opened.mode & 0o111) !== 0
      })
    }
  }
  files.sort((left, right) => Buffer.from(left.path).compare(Buffer.from(right.path)))
  return files
}

async function collectManagedNodeGraph({
  sourcePackageDirectory,
  allowedPackageRoot,
  rootDependencies,
  lockIntegrities
}) {
  const packages = new Map()
  const rootsByPath = new Map()

  async function visit(name, fromDirectory) {
    const resolved = await findPackageRoot(name, fromDirectory, allowedPackageRoot)
    if (rootsByPath.has(resolved.root)) return rootsByPath.get(resolved.root)
    const version = nonEmptyString(resolved.document.version, `${name} package version`)
    const id = `${name}@${version}`
    rootsByPath.set(resolved.root, id)
    const files = await collectNodePackageFiles(resolved.root)
    const required = Object.keys(resolved.document.dependencies ?? {})
    const optional = Object.keys(resolved.document.optionalDependencies ?? {})
    const dependencies = []
    const omittedOptionalDependencies = []
    const builtins = []
    for (const dependency of [...new Set([...required, ...optional])].sort()) {
      if (isBuiltin(dependency)) {
        builtins.push(dependency)
        continue
      }
      try {
        dependencies.push({
          name: dependency,
          package: await visit(dependency, resolved.root),
          optional: optional.includes(dependency) && !required.includes(dependency)
        })
      } catch (error) {
        if (
          optional.includes(dependency) &&
          !required.includes(dependency) &&
          error?.code === 'MANAGED_NODE_DEPENDENCY_NOT_FOUND'
        ) {
          omittedOptionalDependencies.push(dependency)
          continue
        }
        throw error
      }
    }
    const record = {
      id,
      name,
      version,
      integrity: lockedIntegrityForPackage(lockIntegrities, name, version),
      contentRevision: `sha256:${createHash('sha256').update(JSON.stringify(files)).digest('hex')}`,
      dependencies,
      omittedOptionalDependencies,
      builtins,
      files
    }
    const existing = packages.get(id)
    if (existing && JSON.stringify(existing) !== JSON.stringify(record)) {
      throw new Error(`Managed Node package ${id} has inconsistent content or dependency graphs`)
    }
    packages.set(id, record)
    return id
  }

  const roots = []
  for (const dependency of rootDependencies) {
    const packageId = await visit(dependency.name, sourcePackageDirectory)
    if (packageId !== `${dependency.name}@${dependency.version}`) {
      throw new Error(
        `Locked Node dependency ${dependency.name} expected ${dependency.version}, got ${packageId}`
      )
    }
    roots.push({ name: dependency.name, package: packageId })
  }
  const sortedPackages = [...packages.values()].sort((left, right) =>
    Buffer.from(left.id).compare(Buffer.from(right.id))
  )
  return {
    roots,
    packages: sortedPackages,
    graphRevision: `sha256:${createHash('sha256')
      .update(JSON.stringify({ roots, packages: sortedPackages }))
      .digest('hex')}`
  }
}

export async function createManagedNodeDependencyEvidence({
  sourcePackageDirectory = NODE_WORKSPACE_PACKAGE,
  packageManifestPath = join(NODE_WORKSPACE_PACKAGE, 'package.json'),
  lockfilePath = BUILD_INPUT_SOURCE_PATHS.pnpmLockfile,
  rootDependencies,
  allowedPackageRoot
} = {}) {
  const packageManifestBytes = await readFile(packageManifestPath)
  const packageManifest = JSON.parse(packageManifestBytes.toString('utf8'))
  const dependencies =
    rootDependencies ??
    Object.entries(packageManifest.dependencies ?? {}).map(([name, version]) => ({ name, version }))
  const lockfileBytes = await readFile(lockfilePath)
  const boundary = await realpath(
    allowedPackageRoot ?? join(REPOSITORY_ROOT, 'node_modules', '.pnpm')
  )
  const graph = await inspectManagedNodeGraph({
    sourcePackageDirectory,
    allowedPackageRoot: boundary,
    rootDependencies: dependencies,
    lockfilePath
  })
  return {
    schemaVersion: 1,
    lockfile: {
      path: BUILD_INPUT_RELATIVE_PATHS.pnpmLockfile,
      sha256: createHash('sha256').update(lockfileBytes).digest('hex')
    },
    importer: {
      path: BUILD_INPUT_RELATIVE_PATHS.nodePackageManifest,
      sha256: createHash('sha256').update(packageManifestBytes).digest('hex')
    },
    ...graph
  }
}

async function inspectManagedNodeGraph({
  sourcePackageDirectory,
  allowedPackageRoot,
  rootDependencies,
  lockfilePath
}) {
  const lockfileBytes = await readFile(lockfilePath)
  const lockIntegrities = parsePnpmLockPackageIntegrities(lockfileBytes.toString('utf8'))
  return collectManagedNodeGraph({
    sourcePackageDirectory,
    allowedPackageRoot,
    rootDependencies,
    lockIntegrities
  })
}

async function loadManagedNodeDependencyEvidence(manifest) {
  const bytes = await readFile(NODE_PACKAGE_EVIDENCE_SOURCE)
  if (bytes.length === 0 || bytes.length > MAX_NODE_PACKAGE_EVIDENCE_BYTES || bytes.includes(0)) {
    throw new Error('Managed Node dependency evidence is invalid or oversized')
  }
  const evidence = JSON.parse(bytes.toString('utf8'))
  if (
    evidence.schemaVersion !== 1 ||
    evidence.lockfile?.path !== BUILD_INPUT_RELATIVE_PATHS.pnpmLockfile ||
    evidence.lockfile?.sha256 !== manifest.buildInputs.pnpmLockfile.sha256 ||
    evidence.importer?.path !== BUILD_INPUT_RELATIVE_PATHS.nodePackageManifest ||
    evidence.importer?.sha256 !== manifest.buildInputs.nodePackageManifest.sha256 ||
    !Array.isArray(evidence.roots) ||
    !Array.isArray(evidence.packages) ||
    !/^sha256:[a-f0-9]{64}$/.test(evidence.graphRevision)
  ) {
    throw new Error('Managed Node dependency evidence does not match the frozen build inputs')
  }
  return evidence
}

async function copyVerifiedPackageFiles(sourceRoot, destination, packageEvidence) {
  await mkdir(destination, { recursive: true })
  for (const file of packageEvidence.files) {
    const logical = canonicalRelativePath(file.path, `${packageEvidence.id} evidence path`)
    const source = join(sourceRoot, ...logical.split('/'))
    const target = join(destination, ...logical.split('/'))
    const { bytes, metadata } = await readRegularFileNoFollow(
      source,
      `${packageEvidence.id} verified content`
    )
    const digest = createHash('sha256').update(bytes).digest('hex')
    if (
      metadata.size !== file.size ||
      digest !== file.sha256 ||
      ((metadata.mode & 0o111) !== 0) !== file.executable
    ) {
      throw new Error(`Managed Node package content changed after verification: ${source}`)
    }
    await mkdir(dirname(target), { recursive: true })
    await writeFile(target, bytes, {
      flag: 'wx',
      mode: file.executable ? 0o755 : 0o644
    })
  }
}

async function copyPackageDependency(
  name,
  fromDirectory,
  destination,
  allowedPackageRoot,
  evidenceById,
  ancestors = new Set()
) {
  const resolved = await findPackageRoot(name, fromDirectory, allowedPackageRoot)
  if (ancestors.has(resolved.root)) return
  const id = `${resolved.document.name}@${resolved.document.version}`
  const packageEvidence = evidenceById.get(id)
  if (!packageEvidence || packageEvidence.name !== name) {
    throw new Error(`Managed Node dependency ${id} is absent from frozen package evidence`)
  }
  const nextAncestors = new Set(ancestors).add(resolved.root)
  await copyVerifiedPackageFiles(resolved.root, destination, packageEvidence)
  for (const dependency of packageEvidence.dependencies) {
    const childDestination = join(destination, 'node_modules', ...dependency.name.split('/'))
    await copyPackageDependency(
      dependency.name,
      resolved.root,
      childDestination,
      allowedPackageRoot,
      evidenceById,
      nextAncestors
    )
  }
}

export async function prepareManagedNodeDependencies(manifest, staging, options = {}) {
  const expected = options.evidence ?? (await loadManagedNodeDependencyEvidence(manifest))
  const sourcePackageDirectory = options.sourcePackageDirectory ?? NODE_WORKSPACE_PACKAGE
  const allowedPackageRoot = await realpath(
    options.allowedPackageRoot ?? join(REPOSITORY_ROOT, 'node_modules', '.pnpm')
  )
  const lockfilePath = options.lockfilePath ?? BUILD_INPUT_SOURCE_PATHS.pnpmLockfile
  const actualGraph = await inspectManagedNodeGraph({
    sourcePackageDirectory,
    lockfilePath,
    rootDependencies: manifest.node.dependencies,
    allowedPackageRoot
  })
  const expectedGraph = {
    roots: expected.roots,
    packages: expected.packages,
    graphRevision: expected.graphRevision
  }
  if (JSON.stringify(actualGraph) !== JSON.stringify(expectedGraph)) {
    throw new Error(
      'Managed Node dependency graph or package bytes do not match the frozen supply-chain evidence'
    )
  }
  const root = join(staging, ...manifest.node.packageRoot.split('/'))
  await mkdir(root, { recursive: true })
  await writeFile(join(root, 'package.json'), '{"private":true,"type":"module"}\n', {
    flag: 'wx',
    mode: 0o600
  })
  const evidenceById = new Map(expected.packages.map((entry) => [entry.id, entry]))
  for (const dependency of manifest.node.dependencies) {
    const destination = join(root, ...dependency.name.split('/'))
    await copyPackageDependency(
      dependency.name,
      sourcePackageDirectory,
      destination,
      allowedPackageRoot,
      evidenceById
    )
    const document = JSON.parse(await readFile(join(destination, 'package.json'), 'utf8'))
    if (document.name !== dependency.name || document.version !== dependency.version) {
      throw new Error(
        `Locked Node dependency ${dependency.name} expected ${dependency.version}, got ${document.version}`
      )
    }
  }
  const evidenceTarget = join(staging, ...NODE_PACKAGE_EVIDENCE_TARGET.split('/'))
  await mkdir(dirname(evidenceTarget), { recursive: true })
  await writeFile(evidenceTarget, `${JSON.stringify(expected, null, 2)}\n`, {
    flag: 'wx',
    mode: 0o644
  })
}

async function verifyPreparedManagedNodeDependencies(manifest, staging) {
  const expected = await loadManagedNodeDependencyEvidence(manifest)
  const packageRoot = join(staging, ...manifest.node.packageRoot.split('/'))
  const allowedPackageRoot = await realpath(packageRoot)
  const actualGraph = await inspectManagedNodeGraph({
    sourcePackageDirectory: packageRoot,
    allowedPackageRoot,
    rootDependencies: manifest.node.dependencies,
    lockfilePath: BUILD_INPUT_SOURCE_PATHS.pnpmLockfile
  })
  const expectedGraph = {
    roots: expected.roots,
    packages: expected.packages,
    graphRevision: expected.graphRevision
  }
  if (JSON.stringify(actualGraph) !== JSON.stringify(expectedGraph)) {
    throw new Error(
      'Offline Artifact Runtime Node dependencies do not match the frozen supply-chain evidence'
    )
  }
  const publishedEvidence = await readFile(
    join(staging, ...NODE_PACKAGE_EVIDENCE_TARGET.split('/'))
  )
  if (publishedEvidence.toString('utf8') !== `${JSON.stringify(expected, null, 2)}\n`) {
    throw new Error('Offline Artifact Runtime Node package evidence is missing or altered')
  }
}

async function acquirePythonRuntime(manifest, asset, staging, downloadDirectory) {
  const archive = await downloadPinnedFile(asset, archiveCachePath(downloadDirectory, asset))
  const extraction = join(staging, `.python-extract-${randomUUID()}`)
  await mkdir(extraction, { recursive: true })
  const destination = join(staging, ...manifest.python.runtimeHome.split('/'))
  try {
    await extract({ file: archive, cwd: extraction, strict: true, preservePaths: false })
    const source = join(extraction, 'python')
    await copyWithoutSymlinks(source, destination)
  } finally {
    await rm(extraction, { recursive: true, force: true })
  }
  const executableRelative =
    process.platform === 'win32'
      ? manifest.python.executable.win32
      : manifest.python.executable.unix
  const executable = join(staging, ...executableRelative.split('/'))
  if (process.platform !== 'win32') await chmod(executable, 0o755)
  const result = await runProcess(executable, ['--version'], { timeoutMs: 15_000 })
  const reported = `${result.stdout}\n${result.stderr}`.trim()
  if (!reported.includes(`Python ${manifest.python.version}`)) {
    throw new Error(`Managed Python probe returned unexpected version: ${reported}`)
  }
  return executable
}

async function installPythonDependencies(manifest, pythonExecutable) {
  const requirements = join(REPOSITORY_ROOT, ...manifest.python.requirements.split('/'))
  await runProcess(
    pythonExecutable,
    [
      '-I',
      '-m',
      'pip',
      'install',
      '--isolated',
      '--require-hashes',
      '--only-binary=:all:',
      '--no-cache-dir',
      '--no-compile',
      '--no-warn-script-location',
      '--requirement',
      requirements
    ],
    { timeoutMs: 600_000 }
  )
}

async function copyRuntimeSupportFiles(manifestPath, manifest, staging, downloadDirectory) {
  await cp(manifestPath, join(staging, 'runtime-manifest.json'), {
    force: false,
    errorOnExist: true
  })
  const runtimeDirectory = join(staging, 'runtime')
  await mkdir(runtimeDirectory, { recursive: true })
  await cp(NODE_BOOTSTRAP_SOURCE, join(staging, ...manifest.node.bootstrap.split('/')), {
    force: false,
    errorOnExist: true
  })
  await cp(NODE_LOADER_SOURCE, join(staging, ...manifest.node.loader.split('/')), {
    force: false,
    errorOnExist: true
  })
  await verifyPinnedLocalFile(
    join(staging, ...manifest.node.bootstrap.split('/')),
    manifest.buildInputs.nodeBootstrap.sha256,
    'staged managed Node bootstrap'
  )
  await verifyPinnedLocalFile(
    join(staging, ...manifest.node.loader.split('/')),
    manifest.buildInputs.nodeLoader.sha256,
    'staged managed Node loader'
  )
  const legal = await downloadPinnedFile(
    manifest.node.licenseFile,
    archiveCachePath(downloadDirectory, manifest.node.licenseFile)
  )
  const legalTarget = join(staging, ...manifest.node.licenseFile.target.split('/'))
  await mkdir(dirname(legalTarget), { recursive: true })
  await cp(legal, legalTarget, { force: false, errorOnExist: true })
}

function normalizePythonDistributionName(name) {
  return name.toLowerCase().replaceAll('_', '-').replaceAll('.', '-')
}

function legalPackageDirectory(name, version) {
  const readable = `${name}@${version}`.replaceAll('@', '_').replaceAll('/', '__')
  const safe = readable.replaceAll(/[^A-Za-z0-9._+-]/g, '_').slice(0, 120)
  const suffix = createHash('sha256').update(`${name}@${version}`).digest('hex').slice(0, 12)
  return `${safe}-${suffix}`
}

function packageSource(document) {
  const repository =
    typeof document.repository === 'string' ? document.repository : document.repository?.url
  const source = repository ?? document.homepage
  return typeof source === 'string' && source.trim().length > 0 ? source.trim() : null
}

async function copyLegalEvidenceFile(source, destination, componentRoot) {
  const metadata = await lstat(source)
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`Legal evidence must be a regular non-symlink file: ${source}`)
  }
  if (metadata.size <= 0 || metadata.size > MAX_LEGAL_EVIDENCE_FILE_BYTES) {
    throw new Error(`Legal evidence has an invalid size: ${source}`)
  }
  await mkdir(dirname(destination), { recursive: true })
  await cp(source, destination, { force: false, errorOnExist: true })
  return Object.freeze({
    path: destination.slice(componentRoot.length + 1).replaceAll('\\', '/'),
    size: metadata.size,
    sha256: await hashFile(destination)
  })
}

async function collectLicenseFiles(root, { excludeNodeModules = false } = {}) {
  const pending = [{ directory: root, prefix: '' }]
  const found = []
  while (pending.length > 0) {
    const { directory, prefix } = pending.pop()
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      if (excludeNodeModules && entry.name === 'node_modules') continue
      const source = join(directory, entry.name)
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name
      if (entry.isSymbolicLink()) {
        throw new Error(`Legal evidence scan encountered a symlink: ${source}`)
      }
      if (entry.isDirectory()) {
        pending.push({ directory: source, prefix: relative })
      } else if (entry.isFile() && LICENSE_FILE_PATTERN.test(entry.name)) {
        found.push({ source, relative })
        if (found.length > MAX_LEGAL_EVIDENCE_FILES_PER_PACKAGE) {
          throw new Error(`Package contains too many legal evidence files: ${root}`)
        }
      }
    }
  }
  return found.sort((left, right) => left.relative.localeCompare(right.relative, 'en'))
}

async function readNodePackageRoot(packageRoot) {
  const packageJson = join(packageRoot, 'package.json')
  const document = JSON.parse(await readFile(packageJson, 'utf8'))
  if (
    typeof document.name !== 'string' ||
    document.name.length === 0 ||
    typeof document.version !== 'string' ||
    document.version.length === 0
  ) {
    throw new Error(`Managed Node package has invalid identity metadata: ${packageJson}`)
  }
  return { packageRoot, packageJson, document }
}

async function collectNodePackageOccurrences(nodeModulesRoot) {
  const occurrences = []
  async function visitNodeModules(directory) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      if (!entry.isDirectory() || entry.name.startsWith('.')) continue
      if (entry.name.startsWith('@')) {
        const scope = join(directory, entry.name)
        for (const child of await readdir(scope, { withFileTypes: true })) {
          if (child.isDirectory()) await visitPackage(join(scope, child.name))
        }
      } else {
        await visitPackage(join(directory, entry.name))
      }
    }
  }
  async function visitPackage(packageRoot) {
    const packageEntry = await readNodePackageRoot(packageRoot)
    occurrences.push(packageEntry)
    const nested = join(packageRoot, 'node_modules')
    try {
      const metadata = await lstat(nested)
      if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
        throw new Error(`Managed Node node_modules entry is not a regular directory: ${nested}`)
      }
      await visitNodeModules(nested)
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
  }
  await visitNodeModules(nodeModulesRoot)
  return occurrences
}

async function buildNodeLegalInventory(manifest, staging) {
  const nodeModulesRoot = join(staging, ...manifest.node.packageRoot.split('/'))
  const occurrences = await collectNodePackageOccurrences(nodeModulesRoot)
  const grouped = new Map()
  for (const occurrence of occurrences) {
    const key = `${occurrence.document.name}@${occurrence.document.version}`
    const existing = grouped.get(key) ?? []
    existing.push(occurrence)
    grouped.set(key, existing)
  }
  const direct = new Set(
    manifest.node.dependencies.map(({ name, version }) => `${name}@${version}`)
  )
  const inventory = []
  for (const key of [...grouped.keys()].sort()) {
    const packageOccurrences = grouped.get(key)
    const representative = packageOccurrences[0]
    const { document, packageRoot, packageJson } = representative
    const source = packageSource(document)
    if (!source) throw new Error(`Node package ${key} does not declare a source or homepage`)
    const declaredLicense = typeof document.license === 'string' ? document.license : null
    const licenseFiles = await collectLicenseFiles(packageRoot, { excludeNodeModules: true })
    const exception = NODE_METADATA_LICENSE_EVIDENCE_ALLOWLIST[key]
    if (licenseFiles.length === 0) {
      if (!exception || exception.declaredLicense !== declaredLicense) {
        throw new Error(`Node package ${key} has no license file or exact reviewed exception`)
      }
    } else if (!declaredLicense) {
      throw new Error(`Node package ${key} has license files but no declared license expression`)
    }
    const destinationRoot = join(
      staging,
      'legal',
      'node-packages',
      legalPackageDirectory(document.name, document.version)
    )
    const evidence = [
      {
        kind: 'packageMetadata',
        ...(await copyLegalEvidenceFile(
          packageJson,
          join(destinationRoot, 'package.json'),
          staging
        ))
      }
    ]
    for (const candidate of licenseFiles) {
      evidence.push({
        kind: 'licenseFile',
        ...(await copyLegalEvidenceFile(
          candidate.source,
          join(destinationRoot, 'licenses', ...candidate.relative.split('/')),
          staging
        ))
      })
    }
    if (licenseFiles.length === 0) {
      for (const relative of exception.evidenceFiles) {
        if (relative === 'package.json') continue
        evidence.push({
          kind: 'reviewedMetadataEvidence',
          ...(await copyLegalEvidenceFile(
            join(packageRoot, ...relative.split('/')),
            join(destinationRoot, 'reviewed-evidence', ...relative.split('/')),
            staging
          ))
        })
      }
    }
    inventory.push({
      ecosystem: 'npm',
      name: document.name,
      version: document.version,
      direct: direct.has(key),
      licenseExpression: exception?.licenseExpression ?? declaredLicense,
      source,
      installPaths: packageOccurrences
        .map(({ packageRoot: path }) => path.slice(staging.length + 1).replaceAll('\\', '/'))
        .sort(),
      evidence,
      ...(exception ? { reviewedException: exception.review } : {})
    })
  }
  return inventory
}

function parsePythonMetadata(text, label) {
  const values = new Map()
  for (const line of text.split(/\r?\n/)) {
    const separator = line.indexOf(':')
    if (separator <= 0) continue
    const name = line.slice(0, separator).toLowerCase()
    const value = line.slice(separator + 1).trim()
    const existing = values.get(name) ?? []
    existing.push(value)
    values.set(name, existing)
  }
  const first = (name) => values.get(name)?.find((value) => value.length > 0) ?? null
  const name = first('name')
  const version = first('version')
  if (!name || !version) throw new Error(`Python distribution has invalid METADATA: ${label}`)
  const projectUrls = values.get('project-url') ?? []
  const preferredSource = ['source', 'source code', 'repository', 'homepage', 'home', 'code']
    .map((kind) =>
      projectUrls.find((entry) => entry.slice(0, entry.indexOf(',')).trim().toLowerCase() === kind)
    )
    .find(Boolean)
  const source =
    preferredSource?.slice(preferredSource.indexOf(',') + 1).trim() ?? first('home-page')
  return {
    name,
    version,
    licenseExpression: first('license-expression') ?? first('license'),
    source
  }
}

async function findPythonDistributionMetadata(runtimeRoot) {
  const pending = [runtimeRoot]
  const found = []
  while (pending.length > 0) {
    const directory = pending.pop()
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isSymbolicLink()) throw new Error(`Managed Python tree contains a symlink: ${path}`)
      if (!entry.isDirectory()) continue
      if (entry.name.endsWith('.dist-info')) {
        const metadata = join(path, 'METADATA')
        try {
          if ((await lstat(metadata)).isFile()) found.push({ directory: path, metadata })
        } catch (error) {
          if (error?.code !== 'ENOENT') throw error
        }
      } else {
        pending.push(path)
      }
    }
  }
  return found
}

async function buildPythonLegalInventory(manifest, staging) {
  const runtimeRoot = join(staging, ...manifest.python.runtimeHome.split('/'))
  const distributions = await findPythonDistributionMetadata(runtimeRoot)
  const direct = new Set(
    manifest.python.dependencies.map(
      ({ name, version }) => `${normalizePythonDistributionName(name)}@${version}`
    )
  )
  const inventory = []
  const seen = new Set()
  for (const distribution of distributions) {
    const metadataText = await readFile(distribution.metadata, 'utf8')
    const parsed = parsePythonMetadata(metadataText, distribution.metadata)
    const key = `${normalizePythonDistributionName(parsed.name)}@${parsed.version}`
    if (seen.has(key)) throw new Error(`Managed Python contains duplicate distribution ${key}`)
    seen.add(key)
    if (!parsed.source) throw new Error(`Python distribution ${key} does not declare a source`)
    const licenseFiles = await collectLicenseFiles(distribution.directory)
    const exception = PYTHON_METADATA_LICENSE_EVIDENCE_ALLOWLIST[key]
    if (exception && exception.declaredLicense !== parsed.licenseExpression) {
      throw new Error(`Python distribution ${key} no longer matches its reviewed license metadata`)
    }
    const licenseExpression = exception?.licenseExpression ?? parsed.licenseExpression
    if (licenseFiles.length === 0) {
      if (!exception) {
        throw new Error(
          `Python distribution ${key} has no license file or exact reviewed exception`
        )
      }
    } else if (!licenseExpression) {
      throw new Error(`Python distribution ${key} has no declared license expression`)
    }
    const destinationRoot = join(
      staging,
      'legal',
      'python-packages',
      legalPackageDirectory(parsed.name, parsed.version)
    )
    const evidence = [
      {
        kind: 'distributionMetadata',
        ...(await copyLegalEvidenceFile(
          distribution.metadata,
          join(destinationRoot, 'METADATA'),
          staging
        ))
      }
    ]
    for (const candidate of licenseFiles) {
      evidence.push({
        kind: 'licenseFile',
        ...(await copyLegalEvidenceFile(
          candidate.source,
          join(destinationRoot, 'licenses', ...candidate.relative.split('/')),
          staging
        ))
      })
    }
    inventory.push({
      ecosystem: 'pypi',
      name: parsed.name,
      version: parsed.version,
      direct: direct.has(key),
      licenseExpression,
      source: parsed.source,
      installPath: distribution.directory.slice(staging.length + 1).replaceAll('\\', '/'),
      evidence,
      ...(exception ? { reviewedException: exception.review } : {})
    })
  }
  inventory.sort((left, right) =>
    `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`, 'en')
  )
  for (const expected of direct) {
    if (!seen.has(expected))
      throw new Error(`Managed Python is missing direct dependency ${expected}`)
  }
  return inventory
}

export async function prepareArtifactRuntimeLegalEvidence(manifest, staging) {
  await rm(join(staging, 'component-legal.json'), { force: true })
  await rm(join(staging, 'legal', 'node-packages'), { recursive: true, force: true })
  await rm(join(staging, 'legal', 'python-packages'), { recursive: true, force: true })
  await rm(join(staging, 'legal', 'python-runtime'), { recursive: true, force: true })

  const nodeRuntimeLicense = join(staging, ...manifest.node.licenseFile.target.split('/'))
  if (!(await lstat(nodeRuntimeLicense)).isFile()) {
    throw new Error('Managed Node runtime license evidence is missing')
  }
  const pythonRuntimeLicenseSource = join(
    staging,
    ...manifest.python.runtimeHome.split('/'),
    'lib',
    `python${manifest.python.version.split('.').slice(0, 2).join('.')}`,
    'LICENSE.txt'
  )
  const pythonRuntimeLicense = await copyLegalEvidenceFile(
    pythonRuntimeLicenseSource,
    join(staging, 'legal', 'python-runtime', 'CPython-LICENSE.txt'),
    staging
  )
  const nodePackages = await buildNodeLegalInventory(manifest, staging)
  const pythonPackages = await buildPythonLegalInventory(manifest, staging)
  const legalManifest = {
    schemaVersion: 2,
    providerId: manifest.providerId,
    bundleVersion: manifest.bundleVersion,
    runtimes: [
      {
        name: 'Node.js',
        version: manifest.node.version,
        licenseExpression: 'MIT',
        source: 'https://github.com/nodejs/node',
        evidence: [{ kind: 'licenseFile', path: manifest.node.licenseFile.target }]
      },
      {
        name: 'python-build-standalone / CPython',
        version: `${manifest.python.version}+${manifest.python.release}`,
        licenseExpression: manifest.python.license,
        source: manifest.python.source,
        evidence: [
          { kind: 'licenseFile', ...pythonRuntimeLicense },
          {
            kind: 'pinnedManifestMetadata',
            path: 'runtime-manifest.json',
            note: 'python-build-standalone source and MPL-2.0 expression are pinned here'
          }
        ]
      }
    ],
    packages: { node: nodePackages, python: pythonPackages }
  }
  await writeFile(
    join(staging, 'component-legal.json'),
    `${JSON.stringify(legalManifest, null, 2)}\n`,
    { flag: 'wx', mode: 0o600 }
  )
  return legalManifest
}

async function probePreparedRuntimes(manifest, staging, nodeExecutable, pythonExecutable) {
  const nodeVersion = await runProcess(nodeExecutable, ['--version'], { timeoutMs: 15_000 })
  if (nodeVersion.stdout.trim() !== `v${manifest.node.version}`) {
    throw new Error(`Managed Node probe returned unexpected version: ${nodeVersion.stdout.trim()}`)
  }
  const pythonVersion = await runProcess(pythonExecutable, ['--version'], { timeoutMs: 15_000 })
  if (
    !`${pythonVersion.stdout}\n${pythonVersion.stderr}`.includes(
      `Python ${manifest.python.version}`
    )
  ) {
    throw new Error('Managed Python probe returned an unexpected version')
  }
  const packageRoot = join(staging, ...manifest.node.packageRoot.split('/'))
  const bootstrap = join(staging, ...manifest.node.bootstrap.split('/'))
  const nodeProbe = manifest.node.dependencies
    .map((dependency) => `import(${JSON.stringify(dependency.name)})`)
    .join(',')
  await runProcess(
    nodeExecutable,
    ['--import', bootstrap, '--input-type=module', '--eval', `await Promise.all([${nodeProbe}])`],
    {
      timeoutMs: 30_000,
      env: { MYCOPILOT_ARTIFACT_NODE_MODULES: packageRoot }
    }
  )

  const pythonNames = manifest.python.dependencies.map((dependency) => dependency.name)
  const pythonProbe = [
    'import importlib.metadata as m,json',
    `expected=${JSON.stringify(Object.fromEntries(manifest.python.dependencies.map((d) => [d.name, d.version])))}`,
    'actual={name:m.version(name) for name in expected}',
    'assert actual == expected, (actual, expected)',
    'print(json.dumps(actual,sort_keys=True))'
  ].join(';')
  const result = await runProcess(pythonExecutable, ['-I', '-c', pythonProbe], {
    timeoutMs: 30_000
  })
  const actual = JSON.parse(result.stdout.trim())
  for (const name of pythonNames) {
    if (
      actual[name] !== manifest.python.dependencies.find((entry) => entry.name === name).version
    ) {
      throw new Error(`Managed Python dependency probe failed for ${name}`)
    }
  }
}

async function walkRegularFiles(root) {
  const pending = [{ directory: root, prefix: '' }]
  const files = []
  let total = 0
  while (pending.length > 0) {
    const { directory, prefix } = pending.pop()
    const entries = await readdir(directory, { withFileTypes: true })
    entries.sort((left, right) => left.name.localeCompare(right.name, 'en'))
    for (const entry of entries) {
      const path = join(directory, entry.name)
      const logical = prefix ? `${prefix}/${entry.name}` : entry.name
      canonicalRelativePath(logical, 'component file path')
      const metadata = await lstat(path)
      if (metadata.isSymbolicLink()) {
        throw new Error(`Managed Artifact Runtime cannot contain symlink ${logical}`)
      }
      if (metadata.isDirectory()) {
        pending.push({ directory: path, prefix: logical })
        continue
      }
      if (!metadata.isFile()) {
        throw new Error(`Managed Artifact Runtime entry is not a regular file: ${logical}`)
      }
      if (logical === RECEIPT_NAME) continue
      if (files.length === ARTIFACT_RUNTIME_MAX_FILES) {
        throw new Error('Managed Artifact Runtime exceeds the file-count limit')
      }
      total += metadata.size
      if (total > ARTIFACT_RUNTIME_MAX_TOTAL_BYTES) {
        throw new Error('Managed Artifact Runtime exceeds the total byte limit')
      }
      files.push({ path: logical, size: metadata.size, sha256: await hashFile(path) })
    }
  }
  files.sort((left, right) => Buffer.from(left.path).compare(Buffer.from(right.path)))
  return files
}

function buildRuntimeReceipt(manifest, platform) {
  const executable =
    platform === 'win32' ? manifest.node.executable.win32 : manifest.node.executable.unix
  const pythonExecutable =
    platform === 'win32' ? manifest.python.executable.win32 : manifest.python.executable.unix
  const nodeIdentity = [
    executable,
    manifest.node.bootstrap,
    manifest.node.loader,
    NODE_PACKAGE_EVIDENCE_TARGET,
    ...manifest.node.dependencies.map((dependency) => dependency.identityFile)
  ]
  const pythonIdentity = [
    pythonExecutable,
    ...manifest.python.dependencies.map((dependency) => dependency.identityFile)
  ]
  return {
    node: {
      version: manifest.node.version,
      executable,
      packageRoot: manifest.node.packageRoot,
      bootstrap: manifest.node.bootstrap,
      dependencies: manifest.node.dependencies,
      identityFiles: nodeIdentity
    },
    python: {
      version: manifest.python.version,
      executable: pythonExecutable,
      runtimeHome: manifest.python.runtimeHome,
      dependencies: manifest.python.dependencies,
      identityFiles: pythonIdentity
    }
  }
}

function buildReceipt(manifest, platform, arch, files) {
  const runtimes = buildRuntimeReceipt(manifest, platform)
  const buildInputsRevision = `${BUILD_INPUTS_REVISION_PREFIX}${createHash('sha256')
    .update(JSON.stringify(manifest))
    .digest('hex')}`
  const payload = {
    schemaVersion: 2,
    providerId: manifest.providerId,
    bundleVersion: manifest.bundleVersion,
    buildInputsRevision,
    platform,
    arch,
    runtimes,
    files
  }
  const bundleRevision = `${BUNDLE_REVISION_PREFIX}${createHash('sha256')
    .update(JSON.stringify(payload))
    .digest('hex')}`
  return { ...payload, bundleRevision }
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

async function verifyLegalInventory(outputDirectory, manifest, actualFiles) {
  const path = join(outputDirectory, 'component-legal.json')
  const bytes = await readFile(path)
  if (bytes.length <= 0 || bytes.length > 4 * 1024 * 1024 || bytes.includes(0)) {
    throw new Error('Artifact runtime legal inventory is invalid or oversized')
  }
  const legal = JSON.parse(bytes.toString('utf8'))
  if (
    legal.schemaVersion !== 2 ||
    legal.providerId !== manifest.providerId ||
    legal.bundleVersion !== manifest.bundleVersion ||
    !Array.isArray(legal.runtimes) ||
    legal.runtimes.length !== 2 ||
    !legal.packages ||
    !Array.isArray(legal.packages.node) ||
    !Array.isArray(legal.packages.python)
  ) {
    throw new Error('Artifact runtime legal inventory identity or schema is invalid')
  }
  const files = new Map(actualFiles.map((file) => [file.path, file]))
  const inspectEvidence = (owner, evidence) => {
    if (!Array.isArray(evidence) || evidence.length === 0) {
      throw new Error(`Artifact runtime legal inventory has no evidence for ${owner}`)
    }
    for (const item of evidence) {
      const logical = canonicalRelativePath(item.path, `${owner} evidence path`)
      const actual = files.get(logical)
      if (!actual) throw new Error(`Artifact runtime legal evidence is missing: ${logical}`)
      if (item.sha256 !== undefined && item.sha256 !== actual.sha256) {
        throw new Error(`Artifact runtime legal evidence digest mismatch: ${logical}`)
      }
      if (item.size !== undefined && item.size !== actual.size) {
        throw new Error(`Artifact runtime legal evidence size mismatch: ${logical}`)
      }
    }
  }
  for (const runtime of legal.runtimes) {
    nonEmptyString(runtime.name, 'runtime legal name')
    nonEmptyString(runtime.version, 'runtime legal version')
    nonEmptyString(runtime.licenseExpression, 'runtime license expression')
    nonEmptyString(runtime.source, 'runtime source')
    inspectEvidence(`${runtime.name}@${runtime.version}`, runtime.evidence)
  }
  const inspectPackages = (packages, expectedDirect) => {
    const seen = new Set()
    const direct = new Set()
    for (const entry of packages) {
      const name = nonEmptyString(entry.name, 'package legal name')
      const version = nonEmptyString(entry.version, 'package legal version')
      const key = `${normalizePythonDistributionName(name)}@${version}`
      if (seen.has(key)) throw new Error(`Duplicate package in legal inventory: ${key}`)
      seen.add(key)
      if (entry.direct === true) direct.add(key)
      nonEmptyString(entry.licenseExpression, `${key} license expression`)
      nonEmptyString(entry.source, `${key} source`)
      inspectEvidence(key, entry.evidence)
    }
    for (const key of expectedDirect) {
      if (!direct.has(key))
        throw new Error(`Direct package is missing from legal inventory: ${key}`)
    }
  }
  inspectPackages(
    legal.packages.node,
    new Set(
      manifest.node.dependencies.map(
        ({ name, version }) => `${normalizePythonDistributionName(name)}@${version}`
      )
    )
  )
  inspectPackages(
    legal.packages.python,
    new Set(
      manifest.python.dependencies.map(
        ({ name, version }) => `${normalizePythonDistributionName(name)}@${version}`
      )
    )
  )
}

async function verifyReceipt(outputDirectory, manifest, platform, arch) {
  const receiptPath = join(outputDirectory, RECEIPT_NAME)
  const bytes = await readFile(receiptPath)
  if (bytes.length > 16 * 1024 * 1024 || bytes.includes(0)) {
    throw new Error('Artifact runtime receipt is oversized or contains a NUL byte')
  }
  const receipt = JSON.parse(bytes.toString('utf8'))
  if (
    receipt.schemaVersion !== 2 ||
    receipt.providerId !== manifest.providerId ||
    receipt.bundleVersion !== manifest.bundleVersion ||
    receipt.platform !== platform ||
    receipt.arch !== arch
  ) {
    throw new Error('Artifact runtime receipt identity does not match this build target')
  }
  let publishedManifest
  try {
    const publishedManifestBytes = await readFile(join(outputDirectory, 'runtime-manifest.json'))
    if (publishedManifestBytes.length > 1024 * 1024 || publishedManifestBytes.includes(0)) {
      throw new Error('published Artifact Runtime manifest is oversized or contains a NUL byte')
    }
    publishedManifest = validateArtifactRuntimeManifest(
      JSON.parse(publishedManifestBytes.toString('utf8'))
    )
  } catch (error) {
    throw new Error(`Published Artifact Runtime manifest is invalid: ${error.message}`, {
      cause: error
    })
  }
  if (JSON.stringify(publishedManifest) !== JSON.stringify(manifest)) {
    throw new Error('Published Artifact Runtime manifest differs from the current pinned manifest')
  }
  const actualFiles = await walkRegularFiles(outputDirectory)
  if (JSON.stringify(actualFiles) !== JSON.stringify(receipt.files)) {
    throw new Error('Artifact runtime component files do not match the frozen receipt')
  }
  const rebuilt = buildReceipt(manifest, platform, arch, actualFiles)
  if (JSON.stringify(rebuilt.runtimes) !== JSON.stringify(receipt.runtimes)) {
    throw new Error('Artifact runtime receipt runtime descriptors do not match the pinned manifest')
  }
  if (rebuilt.buildInputsRevision !== receipt.buildInputsRevision) {
    throw new Error('Artifact runtime receipt build inputs do not match the pinned manifest')
  }
  if (rebuilt.bundleRevision !== receipt.bundleRevision) {
    throw new Error('Artifact runtime bundle revision does not match its receipt')
  }
  await verifyPinnedLocalFile(
    join(outputDirectory, ...manifest.node.bootstrap.split('/')),
    manifest.buildInputs.nodeBootstrap.sha256,
    'published managed Node bootstrap'
  )
  await verifyPinnedLocalFile(
    join(outputDirectory, ...manifest.node.loader.split('/')),
    manifest.buildInputs.nodeLoader.sha256,
    'published managed Node loader'
  )
  await verifyLegalInventory(outputDirectory, manifest, actualFiles)
  return receipt
}

async function publishDirectoryAtomically(staging, outputDirectory, hooks = {}) {
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
      try {
        await rename(backup, outputDirectory)
      } catch {
        // Preserve the original error. The backup path is intentionally left
        // in place for explicit recovery if restoration itself fails.
      }
    }
    throw error
  }
  if (previousMoved) await rm(backup, { recursive: true, force: true })
}

async function buildComponentSource({ manifestPath, manifest, staging, downloadDirectory }) {
  const { node: nodeAsset, python: pythonAsset } = selectArtifactRuntimeAssets(manifest)
  const nodeExecutable = await acquireNodeRuntime(manifest, nodeAsset, staging, downloadDirectory)
  await prepareManagedNodeDependencies(manifest, staging)
  const pythonExecutable = await acquirePythonRuntime(
    manifest,
    pythonAsset,
    staging,
    downloadDirectory
  )
  await installPythonDependencies(manifest, pythonExecutable)
  await copyRuntimeSupportFiles(manifestPath, manifest, staging, downloadDirectory)
  await probePreparedRuntimes(manifest, staging, nodeExecutable, pythonExecutable)
}

export async function prepareArtifactRuntime({
  manifestPath = DEFAULT_MANIFEST_PATH,
  outputDirectory = DEFAULT_OUTPUT_DIRECTORY,
  downloadDirectory = DEFAULT_DOWNLOAD_DIRECTORY,
  sourceDirectory,
  platform = process.platform,
  arch = process.arch,
  verifyOnly = false,
  forceRebuild = false,
  hooks = {}
} = {}) {
  const manifest = await loadArtifactRuntimeManifest(manifestPath)
  selectArtifactRuntimeAssets(manifest, platform, arch)
  if (verifyOnly) {
    const receipt = await verifyReceipt(outputDirectory, manifest, platform, arch)
    return Object.freeze({ outputDirectory, receipt, reused: true })
  }
  if (!forceRebuild) {
    try {
      const receipt = await verifyReceipt(outputDirectory, manifest, platform, arch)
      return Object.freeze({ outputDirectory, receipt, reused: true })
    } catch {
      // Missing, stale, or damaged components are rebuilt from pinned inputs.
    }
  }

  await mkdir(dirname(outputDirectory), { recursive: true })
  const staging = join(
    dirname(outputDirectory),
    `.${basename(outputDirectory)}.${process.pid}.${randomUUID()}.staging`
  )
  await mkdir(staging, { recursive: false, mode: 0o700 })
  try {
    if (sourceDirectory) {
      const source = resolve(sourceDirectory)
      if (source === resolve(staging) || source === resolve(outputDirectory)) {
        throw new Error('Artifact runtime source directory cannot alias the staging directory')
      }
      await copyTreeRejectingSymlinks(source, staging, { destinationExists: true })
      await verifyPreparedManagedNodeDependencies(manifest, staging)
      const nodeExecutableRelative =
        platform === 'win32' ? manifest.node.executable.win32 : manifest.node.executable.unix
      const pythonExecutableRelative =
        platform === 'win32' ? manifest.python.executable.win32 : manifest.python.executable.unix
      await probePreparedRuntimes(
        manifest,
        staging,
        join(staging, ...nodeExecutableRelative.split('/')),
        join(staging, ...pythonExecutableRelative.split('/'))
      )
    } else {
      await buildComponentSource({ manifestPath, manifest, staging, downloadDirectory })
    }
    await prepareArtifactRuntimeLegalEvidence(manifest, staging)
    const files = await walkRegularFiles(staging)
    const receipt = buildReceipt(manifest, platform, arch, files)
    await writeReceipt(staging, receipt)
    await verifyReceipt(staging, manifest, platform, arch)
    await hooks.afterStaging?.({ staging, receipt })
    await publishDirectoryAtomically(staging, outputDirectory, hooks)
    const published = await verifyReceipt(outputDirectory, manifest, platform, arch)
    return Object.freeze({ outputDirectory, receipt: published, reused: false })
  } finally {
    await rm(staging, { recursive: true, force: true }).catch(() => undefined)
  }
}

function parseArguments(argv) {
  const options = {}
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === '--verify') {
      options.verifyOnly = true
    } else if (argument === '--force') {
      options.forceRebuild = true
    } else if (argument === '--output' || argument === '--source' || argument === '--downloads') {
      const value = argv[index + 1]
      if (!value) throw new Error(`${argument} requires a path`)
      index += 1
      if (argument === '--output') options.outputDirectory = resolve(value)
      if (argument === '--source') options.sourceDirectory = resolve(value)
      if (argument === '--downloads') options.downloadDirectory = resolve(value)
    } else {
      throw new Error(`Unknown argument: ${argument}`)
    }
  }
  return options
}

async function main() {
  const prepared = await prepareArtifactRuntime(parseArguments(process.argv.slice(2)))
  console.log(
    `${prepared.reused ? 'Verified' : 'Prepared'} managed Artifact Runtime ` +
      `${prepared.receipt.bundleVersion} (${prepared.receipt.bundleRevision}) at ${prepared.outputDirectory}`
  )
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined
if (invokedPath === import.meta.url) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  })
}
