/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { createHash, randomUUID } from 'node:crypto'
import { chmod, lstat, open, readFile, readdir, rename, rm } from 'node:fs/promises'
import { basename, dirname, join } from 'node:path'

import { isPinnedRipgrepVersion, runProcess, syncDirectory, unlinkIfPresent } from './archive.mjs'
import { hashFile, verifyPinnedLocalFile } from './filesystem.mjs'
import {
  ARTIFACT_RUNTIME_MAX_FILES,
  ARTIFACT_RUNTIME_MAX_TOTAL_BYTES,
  BUILD_INPUTS_REVISION_PREFIX,
  BUNDLE_REVISION_PREFIX,
  NODE_PACKAGE_EVIDENCE_TARGET,
  RECEIPT_NAME,
  canonicalRelativePath,
  exactKeys,
  nonEmptyString,
  plainObject,
  sha256,
  validateArtifactRuntimeManifest
} from './contract.mjs'
import { normalizePythonDistributionName } from './legal-evidence.mjs'

export async function probePreparedRuntimes(
  manifest,
  staging,
  nodeExecutable,
  pythonExecutable,
  ripgrepExecutable
) {
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
  // -I ignores PYTHON* environment variables, so -B must be explicit here as well.
  const result = await runProcess(pythonExecutable, ['-I', '-B', '-c', pythonProbe], {
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
  const ripgrepVersion = await runProcess(ripgrepExecutable, ['--version'], { timeoutMs: 15_000 })
  if (!isPinnedRipgrepVersion(ripgrepVersion.stdout, manifest.tools.ripgrep.version)) {
    throw new Error(
      `Managed ripgrep probe returned unexpected version: ${ripgrepVersion.stdout.trim()}`
    )
  }
  const pdfCli = join(staging, ...manifest.tools.pdfCli.target.split('/'))
  await verifyPinnedLocalFile(
    pdfCli,
    manifest.buildInputs.pdfRuntimeCli.sha256,
    'prepared managed PDF CLI'
  )
}

export async function walkRegularFiles(root) {
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
    manifest.node.presentationSdk,
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

function buildToolReceipt(manifest, platform) {
  const ripgrepExecutable =
    platform === 'win32'
      ? manifest.tools.ripgrep.executable.win32
      : manifest.tools.ripgrep.executable.unix
  return {
    pdfCli: {
      version: manifest.tools.pdfCli.version,
      path: manifest.tools.pdfCli.target,
      identityFiles: [manifest.tools.pdfCli.target]
    },
    ripgrep: {
      version: manifest.tools.ripgrep.version,
      executable: ripgrepExecutable,
      identityFiles: [
        ripgrepExecutable,
        ...manifest.tools.ripgrep.licenseFiles.map(({ target }) => target)
      ]
    }
  }
}

export function buildReceipt(manifest, platform, arch, files) {
  const runtimes = buildRuntimeReceipt(manifest, platform)
  const tools = buildToolReceipt(manifest, platform)
  const buildInputsRevision = `${BUILD_INPUTS_REVISION_PREFIX}${createHash('sha256')
    .update(JSON.stringify(manifest))
    .digest('hex')}`
  const payload = {
    schemaVersion: 3,
    providerId: manifest.providerId,
    bundleVersion: manifest.bundleVersion,
    buildInputsRevision,
    platform,
    arch,
    runtimes,
    tools,
    files
  }
  const bundleRevision = `${BUNDLE_REVISION_PREFIX}${createHash('sha256')
    .update(JSON.stringify(payload))
    .digest('hex')}`
  return { ...payload, bundleRevision }
}

export async function writeReceipt(staging, receipt) {
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

export async function replaceReceiptAtomically(outputDirectory, receipt) {
  const receiptPath = join(outputDirectory, RECEIPT_NAME)
  const temporaryPath = join(outputDirectory, `.${RECEIPT_NAME}.${process.pid}.${randomUUID()}.tmp`)
  let handle
  try {
    handle = await open(temporaryPath, 'wx', 0o600)
    await handle.writeFile(`${JSON.stringify(receipt, null, 2)}\n`)
    await handle.sync()
    await handle.close()
    handle = undefined
    if (process.platform !== 'win32') await chmod(temporaryPath, 0o644)
    await rename(temporaryPath, receiptPath)
    await syncDirectory(outputDirectory)
  } finally {
    await handle?.close().catch(() => undefined)
    await unlinkIfPresent(temporaryPath)
  }
}

async function verifyLegalInventory(outputDirectory, manifest, actualFiles) {
  const path = join(outputDirectory, 'component-legal.json')
  const bytes = await readFile(path)
  if (bytes.length <= 0 || bytes.length > 4 * 1024 * 1024 || bytes.includes(0)) {
    throw new Error('Artifact runtime legal inventory is invalid or oversized')
  }
  const legal = JSON.parse(bytes.toString('utf8'))
  if (
    legal.schemaVersion !== 3 ||
    legal.providerId !== manifest.providerId ||
    legal.bundleVersion !== manifest.bundleVersion ||
    !Array.isArray(legal.runtimes) ||
    legal.runtimes.length !== 2 ||
    !Array.isArray(legal.tools) ||
    legal.tools.length !== 1 ||
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
  const [ripgrep] = legal.tools
  if (
    ripgrep.name !== 'ripgrep' ||
    ripgrep.version !== manifest.tools.ripgrep.version ||
    ripgrep.licenseExpression !== manifest.tools.ripgrep.license ||
    ripgrep.source !== manifest.tools.ripgrep.source
  ) {
    throw new Error('Artifact runtime legal inventory has an invalid ripgrep identity')
  }
  inspectEvidence(`ripgrep@${ripgrep.version}`, ripgrep.evidence)
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

export async function verifyReceipt(outputDirectory, manifest, platform, arch) {
  const receiptPath = join(outputDirectory, RECEIPT_NAME)
  const receiptMetadata = await lstat(receiptPath)
  if (!receiptMetadata.isFile() || receiptMetadata.isSymbolicLink()) {
    throw new Error('Artifact runtime receipt must be a regular non-symlink file')
  }
  const bytes = await readFile(receiptPath)
  if (bytes.length <= 0 || bytes.length > 16 * 1024 * 1024 || bytes.includes(0)) {
    throw new Error('Artifact runtime receipt is empty, oversized, or contains a NUL byte')
  }
  const receipt = JSON.parse(bytes.toString('utf8'))
  if (
    receipt.schemaVersion !== 3 ||
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
  if (JSON.stringify(rebuilt.tools) !== JSON.stringify(receipt.tools)) {
    throw new Error('Artifact runtime receipt tool descriptors do not match the pinned manifest')
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
  await verifyPinnedLocalFile(
    join(outputDirectory, ...manifest.node.presentationSdk.split('/')),
    manifest.buildInputs.presentationSdk.sha256,
    'published Presentation Editor SDK'
  )
  await verifyPinnedLocalFile(
    join(outputDirectory, ...manifest.tools.pdfCli.target.split('/')),
    manifest.buildInputs.pdfRuntimeCli.sha256,
    'published managed PDF CLI'
  )
  if (platform !== 'win32') {
    const ripgrepExecutable = manifest.tools.ripgrep.executable.unix
    for (const relative of [ripgrepExecutable]) {
      const metadata = await lstat(join(outputDirectory, ...relative.split('/')))
      if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o111) === 0) {
        throw new Error(
          `Published managed tool entry is not an executable regular file: ${relative}`
        )
      }
    }
  }
  await verifyLegalInventory(outputDirectory, manifest, actualFiles)
  return receipt
}

export function validateFrozenArtifactRuntimeReceipt(receipt, manifest, platform, arch) {
  const value = plainObject(receipt, 'Artifact runtime frozen receipt')
  exactKeys(
    value,
    [
      'schemaVersion',
      'providerId',
      'bundleVersion',
      'buildInputsRevision',
      'platform',
      'arch',
      'runtimes',
      'tools',
      'files',
      'bundleRevision'
    ],
    'Artifact runtime frozen receipt'
  )
  if (!Array.isArray(value.files) || value.files.length === 0) {
    throw new Error('Artifact runtime frozen receipt must contain files')
  }
  if (value.files.length > ARTIFACT_RUNTIME_MAX_FILES) {
    throw new Error('Artifact runtime frozen receipt exceeds the file-count limit')
  }
  let priorPath
  let totalBytes = 0
  for (const [index, file] of value.files.entries()) {
    const descriptor = plainObject(file, `Artifact runtime frozen receipt.files[${index}]`)
    exactKeys(
      descriptor,
      ['path', 'size', 'sha256'],
      `Artifact runtime frozen receipt.files[${index}]`
    )
    const path = canonicalRelativePath(
      descriptor.path,
      `Artifact runtime frozen receipt.files[${index}].path`
    )
    if (priorPath !== undefined && Buffer.compare(Buffer.from(priorPath), Buffer.from(path)) >= 0) {
      throw new Error('Artifact runtime frozen receipt files must be uniquely sorted by path')
    }
    priorPath = path
    if (!Number.isSafeInteger(descriptor.size) || descriptor.size < 0) {
      throw new Error('Artifact runtime frozen receipt contains an invalid file size')
    }
    totalBytes += descriptor.size
    if (totalBytes > ARTIFACT_RUNTIME_MAX_TOTAL_BYTES) {
      throw new Error('Artifact runtime frozen receipt exceeds the total byte limit')
    }
    sha256(descriptor.sha256, `Artifact runtime frozen receipt.files[${index}].sha256`)
  }
  const rebuilt = buildReceipt(manifest, platform, arch, value.files)
  if (JSON.stringify(rebuilt) !== JSON.stringify(value)) {
    throw new Error('Artifact runtime frozen receipt does not match the pinned manifest')
  }
  return value
}

export async function publishDirectoryAtomically(staging, outputDirectory, hooks = {}) {
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
