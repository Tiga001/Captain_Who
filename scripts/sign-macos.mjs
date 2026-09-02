/* eslint-disable @typescript-eslint/explicit-function-return-type -- electron-builder loads this JavaScript module directly. */

import { execFile as execFileCallback } from 'node:child_process'
import { createHash } from 'node:crypto'
import { lstat, open, readdir } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { promisify } from 'node:util'
import { fileURLToPath } from 'node:url'

import {
  prepareOfficeRenderer,
  refreshOfficeRendererReceiptAfterSigning
} from './prepare-office-renderer.mjs'
import {
  prepareArtifactRuntimeMacSigning,
  refreshArtifactRuntimeReceiptAfterSigning
} from './prepare-artifact-runtime.mjs'
import {
  prepareOfficeCliMacSigning,
  refreshOfficeCliReceiptAfterSigning
} from './prepare-officecli.mjs'

export const APP_CODE_SIGN_IDENTIFIER = 'io.github.tiga001.captainwho'
export const CORE_SERVER_CODE_SIGN_IDENTIFIER = `${APP_CODE_SIGN_IDENTIFIER}.core-server`
export const OFFICE_RENDERER_CODE_SIGN_IDENTIFIERS = Object.freeze({
  'libEGL.dylib': `${APP_CODE_SIGN_IDENTIFIER}.office-renderer.libegl`,
  'libGLESv2.dylib': `${APP_CODE_SIGN_IDENTIFIER}.office-renderer.libglesv2`,
  'libvk_swiftshader.dylib': `${APP_CODE_SIGN_IDENTIFIER}.office-renderer.libvk-swiftshader`,
  'chrome-headless-shell': `${APP_CODE_SIGN_IDENTIFIER}.office-renderer.chrome-headless-shell`
})
export const OFFICECLI_CODE_SIGN_IDENTIFIER = `${APP_CODE_SIGN_IDENTIFIER}.officecli`

const execFile = promisify(execFileCallback)
const CORE_SERVER_RELATIVE_PATH = join('Contents', 'Resources', 'core-server')
const OFFICE_RENDERER_RELATIVE_PATH = join('Contents', 'Resources', 'components', 'office-renderer')
const OFFICECLI_RELATIVE_PATH = join('Contents', 'Resources', 'components', 'officecli')
const ARTIFACT_RUNTIME_RELATIVE_PATH = join(
  'Contents',
  'Resources',
  'components',
  'artifact-runtime'
)
const CORE_SERVER_ENTITLEMENTS = resolve(
  dirname(fileURLToPath(import.meta.url)),
  '..',
  'build',
  'entitlements.core-server.mac.plist'
)
const JIT_RUNTIME_ENTITLEMENTS = resolve(
  dirname(fileURLToPath(import.meta.url)),
  '..',
  'build',
  'entitlements.jit-runtime.mac.plist'
)

function requireSigningConfiguration(configuration) {
  if (!configuration || typeof configuration !== 'object') {
    throw new Error('macOS signing configuration is required')
  }
  if (configuration.platform !== 'darwin') {
    throw new Error(
      `Captain Who distribution signing requires darwin, got ${configuration.platform}`
    )
  }
  if (typeof configuration.app !== 'string' || !configuration.app.endsWith('.app')) {
    throw new Error('Captain Who distribution signing requires a macOS .app path')
  }
  if (
    typeof configuration.identity !== 'string' ||
    configuration.identity.length === 0 ||
    configuration.identity === '-'
  ) {
    throw new Error(
      'Captain Who distribution signing requires a real Developer ID Application identity; ad-hoc and unsigned identities are forbidden'
    )
  }
}

function hasIdentifierArgument(argumentsList) {
  return argumentsList.some(
    (argument) => argument === '--identifier' || argument.startsWith('--identifier=')
  )
}

export function isCoreServerSigningTarget(appPath, filePath) {
  if (typeof appPath !== 'string' || typeof filePath !== 'string') {
    return false
  }
  return resolve(filePath) === resolve(appPath, CORE_SERVER_RELATIVE_PATH)
}

export function officeRendererMacCodeSigningTargets(appPath, receipt) {
  if (typeof appPath !== 'string' || !appPath.endsWith('.app')) {
    throw new Error('Office renderer signing requires a macOS .app path')
  }
  const executable = receipt?.browser?.executable
  const match =
    typeof executable === 'string'
      ? executable.match(
          /^browser\/(chrome-headless-shell-mac-(?:arm64|x64))\/chrome-headless-shell$/
        )
      : null
  if (!match) {
    throw new Error('Office renderer receipt has an unexpected macOS Chromium executable path')
  }
  if (!Array.isArray(receipt.files)) {
    throw new Error('Office renderer receipt must contain a frozen file set')
  }

  const browserDirectory = `browser/${match[1]}`
  const frozenPaths = new Set(receipt.files.map((file) => file?.path))
  const componentRoot = resolve(appPath, OFFICE_RENDERER_RELATIVE_PATH)
  return Object.entries(OFFICE_RENDERER_CODE_SIGN_IDENTIFIERS).map(([name, identifier]) => {
    const relativePath = `${browserDirectory}/${name}`
    if (!frozenPaths.has(relativePath)) {
      throw new Error(`Office renderer receipt is missing required macOS code: ${relativePath}`)
    }
    return Object.freeze({
      relativePath,
      path: resolve(componentRoot, ...relativePath.split('/')),
      identifier,
      entitlements: name === 'chrome-headless-shell' ? JIT_RUNTIME_ENTITLEMENTS : undefined
    })
  })
}

export function frozenMachOCodeSignIdentifier(component, relativePath) {
  if (component === 'officecli') {
    if (relativePath !== 'officecli') {
      throw new Error('OfficeCLI has an unexpected frozen Mach-O path')
    }
    return OFFICECLI_CODE_SIGN_IDENTIFIER
  }
  if (component !== 'artifact-runtime') {
    throw new Error(`Unsupported frozen component code-sign identifier: ${String(component)}`)
  }
  if (
    typeof relativePath !== 'string' ||
    relativePath.length === 0 ||
    relativePath.startsWith('/') ||
    relativePath.includes('\\') ||
    relativePath.split('/').some((part) => part.length === 0 || part === '.' || part === '..')
  ) {
    throw new Error('Artifact Runtime code-sign path must be canonical and relative')
  }
  const digest = createHash('sha256').update(relativePath, 'utf8').digest('hex').slice(0, 24)
  return `${APP_CODE_SIGN_IDENTIFIER}.artifact-runtime.${digest}`
}

export function artifactRuntimeMacCodeSigningTargets(targets) {
  if (!Array.isArray(targets) || targets.length === 0) {
    throw new Error('Artifact Runtime signing requires at least one frozen Mach-O target')
  }
  const mapped = targets.map((target) => ({
    ...target,
    identifier: frozenMachOCodeSignIdentifier('artifact-runtime', target.relativePath),
    entitlements:
      target.relativePath === 'dependencies/node/bin/node' ? JIT_RUNTIME_ENTITLEMENTS : undefined
  }))
  // Sign libraries and extension modules before the executables that load them. The receipt paths
  // remain the canonical transaction order, independent of filesystem traversal order.
  mapped.sort((left, right) => {
    const leftLibrary = /\.(?:dylib|so)$/.test(left.relativePath)
    const rightLibrary = /\.(?:dylib|so)$/.test(right.relativePath)
    if (leftLibrary !== rightLibrary) return leftLibrary ? -1 : 1
    return Buffer.from(left.relativePath).compare(Buffer.from(right.relativePath))
  })
  return Object.freeze(mapped.map((target) => Object.freeze(target)))
}

export function officeCliMacCodeSigningTargets(targets) {
  if (!Array.isArray(targets) || targets.length !== 1 || targets[0]?.relativePath !== 'officecli') {
    throw new Error('OfficeCLI signing requires its exact frozen executable')
  }
  return Object.freeze([
    Object.freeze({
      ...targets[0],
      identifier: frozenMachOCodeSignIdentifier('officecli', targets[0].relativePath),
      // OfficeCLI embeds CoreCLR and fails to start under hardened runtime without JIT permission.
      entitlements: JIT_RUNTIME_ENTITLEMENTS
    })
  ])
}

export async function inspectFrozenMachOArchitectures({ targets, expectedArch, run = execFile }) {
  if (!Array.isArray(targets) || targets.length === 0) {
    throw new Error('Frozen component architecture inspection requires Mach-O targets')
  }
  if (!['arm64', 'x64'].includes(expectedArch)) {
    throw new Error(`Frozen component has an unsupported signing architecture: ${expectedArch}`)
  }
  if (typeof run !== 'function') {
    throw new Error('Frozen component architecture inspection requires a command runner')
  }
  const expectedLipoArch = expectedArch === 'x64' ? 'x86_64' : expectedArch
  const allowedArchitectures = new Set(['arm64', 'x86_64'])
  const inspected = []
  for (const target of targets) {
    const result = await run('/usr/bin/lipo', ['-archs', target.path], {
      encoding: 'utf8',
      maxBuffer: 1024 * 1024
    })
    const architectures = String(result.stdout ?? '')
      .trim()
      .split(/\s+/)
      .filter(Boolean)
    if (
      architectures.length === 0 ||
      !architectures.includes(expectedLipoArch) ||
      architectures.some((architecture) => !allowedArchitectures.has(architecture))
    ) {
      throw new Error(
        `Frozen Mach-O architecture mismatch for ${target.relativePath}: expected ${expectedLipoArch}`
      )
    }
    inspected.push(Object.freeze({ target, architectures: Object.freeze(architectures) }))
  }
  return Object.freeze(inspected)
}

function machOArchitecture(header) {
  if (header.length < 8) return null
  const magic = header.subarray(0, 4).toString('hex')
  let byteOrder
  if (magic === 'cffaedfe' || magic === 'cefaedfe') byteOrder = 'little'
  else if (magic === 'feedfacf' || magic === 'feedface') byteOrder = 'big'
  else if (['cafebabe', 'bebafeca', 'cafebabf', 'bfbafeca'].includes(magic)) return 'universal'
  else return null
  const cpuType = byteOrder === 'little' ? header.readUInt32LE(4) : header.readUInt32BE(4)
  if (cpuType === 0x0100000c) return 'arm64'
  if (cpuType === 0x01000007) return 'x64'
  return `unsupported-${cpuType.toString(16)}`
}

async function readMachOArchitecture(path) {
  const handle = await open(path, 'r')
  try {
    const header = Buffer.alloc(8)
    const { bytesRead } = await handle.read(header, 0, header.length, 0)
    return machOArchitecture(header.subarray(0, bytesRead))
  } finally {
    await handle.close()
  }
}

export async function inspectOfficeRendererMachO({ outputDirectory, targets, expectedArch }) {
  if (!['arm64', 'x64'].includes(expectedArch)) {
    throw new Error(
      `Office renderer has an unsupported signing architecture: ${String(expectedArch)}`
    )
  }
  const targetByPath = new Map(targets.map((target) => [resolve(target.path), target]))
  if (targetByPath.size !== 4 || targets.length !== 4) {
    throw new Error('Office renderer signing allowlist must contain exactly four paths')
  }

  const discovered = []
  const pending = [resolve(outputDirectory)]
  while (pending.length > 0) {
    const directory = pending.pop()
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = resolve(directory, entry.name)
      const metadata = await lstat(path)
      if (metadata.isSymbolicLink()) {
        throw new Error('Office renderer signing boundary cannot contain symbolic links')
      }
      if (metadata.isDirectory()) {
        pending.push(path)
        continue
      }
      if (!metadata.isFile()) {
        throw new Error('Office renderer signing boundary can contain only regular files')
      }
      const architecture = await readMachOArchitecture(path)
      if (architecture !== null) discovered.push({ path, architecture })
    }
  }

  const discoveredByPath = new Map(discovered.map((entry) => [entry.path, entry]))
  if (
    discoveredByPath.size !== targetByPath.size ||
    [...discoveredByPath.keys()].some((path) => !targetByPath.has(path))
  ) {
    throw new Error('Office renderer Mach-O set does not match its exact four-file allowlist')
  }
  for (const targetPath of targetByPath.keys()) {
    const discoveredTarget = discoveredByPath.get(targetPath)
    if (discoveredTarget?.architecture !== expectedArch) {
      throw new Error(
        `Office renderer Mach-O architecture mismatch: expected ${expectedArch}, got ${String(discoveredTarget?.architecture)}`
      )
    }
  }
  return Object.freeze(discovered)
}

export function createMacSignOptions(configuration) {
  requireSigningConfiguration(configuration)

  const inheritedOptionsForFile = configuration.optionsForFile
  if (inheritedOptionsForFile !== undefined && typeof inheritedOptionsForFile !== 'function') {
    throw new Error('macOS signing optionsForFile must be a function when provided')
  }

  return {
    ...configuration,
    strictVerify: true,
    optionsForFile(filePath) {
      const inheritedOptions = inheritedOptionsForFile?.(filePath) ?? null
      if (
        inheritedOptions !== null &&
        (typeof inheritedOptions !== 'object' ||
          typeof inheritedOptions.then === 'function' ||
          Array.isArray(inheritedOptions))
      ) {
        throw new Error('macOS signing optionsForFile must return an object or null synchronously')
      }
      if (!isCoreServerSigningTarget(configuration.app, filePath)) {
        return inheritedOptions
      }

      const additionalArguments = [...(inheritedOptions?.additionalArguments ?? [])]
      if (hasIdentifierArgument(additionalArguments)) {
        throw new Error('core-server signing options already contain a code-sign identifier')
      }

      return {
        ...inheritedOptions,
        entitlements: CORE_SERVER_ENTITLEMENTS,
        hardenedRuntime: true,
        additionalArguments: [
          ...additionalArguments,
          '--identifier',
          CORE_SERVER_CODE_SIGN_IDENTIFIER
        ]
      }
    }
  }
}

export function resolvePromiseSigningFunction(osxSignModule) {
  const defaultExport = osxSignModule?.default
  const signApplication =
    osxSignModule?.signAsync ??
    osxSignModule?.signApp ??
    defaultExport?.signAsync ??
    defaultExport?.signApp
  if (typeof signApplication !== 'function') {
    throw new Error('@electron/osx-sign did not expose its Promise-based signing function')
  }
  return signApplication
}

function codesignArguments(configuration, target) {
  const argumentsList = [
    '--sign',
    configuration.identity,
    '--force',
    '--timestamp',
    '--options',
    'runtime',
    '--identifier',
    target.identifier
  ]
  if (configuration.keychain !== undefined) {
    if (typeof configuration.keychain !== 'string' || configuration.keychain.length === 0) {
      throw new Error('macOS signing keychain must be a non-empty path when provided')
    }
    argumentsList.push('--keychain', configuration.keychain)
  }
  if (target.entitlements !== undefined) {
    argumentsList.push('--entitlements', target.entitlements)
  }
  argumentsList.push(target.path)
  return argumentsList
}

function parseSignedCodeMetadata(output) {
  const field = (name) => output.match(new RegExp(`^${name}=(.+)$`, 'm'))?.[1].trim()
  return {
    identifier: field('Identifier'),
    teamIdentifier: field('TeamIdentifier'),
    authority: output.match(/^Authority=(.+)$/m)?.[1].trim(),
    adHoc: /^Signature=adhoc$/m.test(output),
    runtime: /^CodeDirectory .*\bflags=.*\bruntime\b/m.test(output),
    timestamp: field('Timestamp')
  }
}

function assertOfficeRendererDeveloperIdMetadata(target, metadata, expectedSigner) {
  if (metadata.identifier !== target.identifier) {
    throw new Error(`Office renderer Mach-O has an unexpected identifier: ${target.relativePath}`)
  }
  if (
    metadata.adHoc ||
    !metadata.authority?.startsWith('Developer ID Application:') ||
    !metadata.teamIdentifier ||
    metadata.teamIdentifier === 'not set'
  ) {
    throw new Error(`Office renderer Mach-O is not Developer ID signed: ${target.relativePath}`)
  }
  if (!metadata.runtime || !metadata.timestamp || metadata.timestamp === 'none') {
    throw new Error(
      `Office renderer Mach-O lacks hardened runtime or timestamp: ${target.relativePath}`
    )
  }
  if (
    expectedSigner !== undefined &&
    (metadata.teamIdentifier !== expectedSigner.teamIdentifier ||
      metadata.authority !== expectedSigner.authority)
  ) {
    throw new Error('Office renderer Mach-O files are not signed by one Developer ID identity')
  }
  return Object.freeze({
    teamIdentifier: metadata.teamIdentifier,
    authority: metadata.authority
  })
}

function assertFrozenDeveloperIdMetadata(componentLabel, target, metadata, expectedSigner) {
  if (metadata.identifier !== target.identifier) {
    throw new Error(`${componentLabel} Mach-O has an unexpected identifier: ${target.relativePath}`)
  }
  if (
    metadata.adHoc ||
    !metadata.authority?.startsWith('Developer ID Application:') ||
    !metadata.teamIdentifier ||
    metadata.teamIdentifier === 'not set'
  ) {
    throw new Error(`${componentLabel} Mach-O is not Developer ID signed: ${target.relativePath}`)
  }
  if (!metadata.runtime || !metadata.timestamp || metadata.timestamp === 'none') {
    throw new Error(
      `${componentLabel} Mach-O lacks hardened runtime or timestamp: ${target.relativePath}`
    )
  }
  if (
    expectedSigner !== undefined &&
    (metadata.teamIdentifier !== expectedSigner.teamIdentifier ||
      metadata.authority !== expectedSigner.authority)
  ) {
    throw new Error(`${componentLabel} Mach-O files use different Developer ID identities`)
  }
  return Object.freeze({
    teamIdentifier: metadata.teamIdentifier,
    authority: metadata.authority
  })
}

async function signFrozenMachOTargets(configuration, { componentLabel, targets, run }) {
  let expectedSigner
  for (const target of targets) {
    await run('/usr/bin/codesign', codesignArguments(configuration, target), {
      encoding: 'utf8',
      maxBuffer: 1024 * 1024
    })
    await run('/usr/bin/codesign', ['--verify', '--strict', '--verbose=2', target.path], {
      encoding: 'utf8',
      maxBuffer: 1024 * 1024
    })
    const details = await run('/usr/bin/codesign', ['--display', '--verbose=4', target.path], {
      encoding: 'utf8',
      maxBuffer: 1024 * 1024
    })
    const metadata = parseSignedCodeMetadata(`${details.stdout ?? ''}\n${details.stderr ?? ''}`)
    expectedSigner = assertFrozenDeveloperIdMetadata(
      componentLabel,
      target,
      metadata,
      expectedSigner
    )
  }
}

export async function signArtifactRuntimeMachO(
  configuration,
  {
    run = execFile,
    prepare = prepareArtifactRuntimeMacSigning,
    inspect = inspectFrozenMachOArchitectures,
    refreshReceipt = refreshArtifactRuntimeReceiptAfterSigning
  } = {}
) {
  requireSigningConfiguration(configuration)
  if (
    typeof run !== 'function' ||
    typeof prepare !== 'function' ||
    typeof inspect !== 'function' ||
    typeof refreshReceipt !== 'function'
  ) {
    throw new Error('Artifact Runtime signing dependencies must be functions')
  }
  const outputDirectory = resolve(configuration.app, ARTIFACT_RUNTIME_RELATIVE_PATH)
  const prepared = await prepare({
    outputDirectory,
    platform: 'darwin',
    arch: process.arch
  })
  const targets = artifactRuntimeMacCodeSigningTargets(prepared.targets)
  await inspect({ targets, expectedArch: prepared.receipt.arch, run })
  await signFrozenMachOTargets(configuration, {
    componentLabel: 'Artifact Runtime',
    targets,
    run
  })
  const receipt = await refreshReceipt({
    outputDirectory,
    originalReceipt: prepared.receipt,
    signedPaths: targets.map((target) => target.relativePath),
    platform: 'darwin',
    arch: process.arch
  })
  return Object.freeze({ outputDirectory, receipt, targets })
}

export async function signOfficeCliMachO(
  configuration,
  {
    run = execFile,
    prepare = prepareOfficeCliMacSigning,
    inspect = inspectFrozenMachOArchitectures,
    refreshReceipt = refreshOfficeCliReceiptAfterSigning
  } = {}
) {
  requireSigningConfiguration(configuration)
  if (
    typeof run !== 'function' ||
    typeof prepare !== 'function' ||
    typeof inspect !== 'function' ||
    typeof refreshReceipt !== 'function'
  ) {
    throw new Error('OfficeCLI signing dependencies must be functions')
  }
  const outputDirectory = resolve(configuration.app, OFFICECLI_RELATIVE_PATH)
  const prepared = await prepare({
    outputDirectory,
    platform: 'darwin',
    arch: process.arch
  })
  const targets = officeCliMacCodeSigningTargets(prepared.targets)
  await inspect({ targets, expectedArch: prepared.receipt.arch, run })
  await signFrozenMachOTargets(configuration, { componentLabel: 'OfficeCLI', targets, run })
  const receipt = await refreshReceipt({
    outputDirectory,
    originalReceipt: prepared.receipt,
    signedPaths: targets.map((target) => target.relativePath),
    platform: 'darwin',
    arch: process.arch
  })
  return Object.freeze({ outputDirectory, receipt, targets })
}

export async function signOfficeRendererMachO(
  configuration,
  {
    run = execFile,
    prepare = prepareOfficeRenderer,
    inspect = inspectOfficeRendererMachO,
    refreshReceipt = refreshOfficeRendererReceiptAfterSigning
  } = {}
) {
  requireSigningConfiguration(configuration)
  if (
    typeof run !== 'function' ||
    typeof prepare !== 'function' ||
    typeof inspect !== 'function' ||
    typeof refreshReceipt !== 'function'
  ) {
    throw new Error('Office renderer signing dependencies must be functions')
  }

  const outputDirectory = resolve(configuration.app, OFFICE_RENDERER_RELATIVE_PATH)
  const prepared = await prepare({
    outputDirectory,
    verifyOnly: true,
    platform: 'darwin',
    arch: process.arch
  })
  const targets = officeRendererMacCodeSigningTargets(configuration.app, prepared.receipt)
  await inspect({
    outputDirectory,
    targets,
    expectedArch: prepared.receipt.arch
  })
  let expectedSigner
  for (const target of targets) {
    await run('/usr/bin/codesign', codesignArguments(configuration, target), {
      encoding: 'utf8',
      maxBuffer: 1024 * 1024
    })
    await run('/usr/bin/codesign', ['--verify', '--strict', '--verbose=2', target.path], {
      encoding: 'utf8',
      maxBuffer: 1024 * 1024
    })
    const details = await run('/usr/bin/codesign', ['--display', '--verbose=4', target.path], {
      encoding: 'utf8',
      maxBuffer: 1024 * 1024
    })
    const metadata = parseSignedCodeMetadata(`${details.stdout ?? ''}\n${details.stderr ?? ''}`)
    expectedSigner = assertOfficeRendererDeveloperIdMetadata(target, metadata, expectedSigner)
  }
  const receipt = await refreshReceipt({
    outputDirectory,
    originalReceipt: prepared.receipt,
    signedPaths: targets.map((target) => target.relativePath),
    platform: 'darwin',
    arch: process.arch
  })
  return Object.freeze({ outputDirectory, receipt, targets })
}

export async function sign(configuration) {
  const options = createMacSignOptions(configuration)
  // These frozen components are excluded from osx-sign's recursive pass. Verify their original
  // receipts, sign only the complete Mach-O allowlists, refresh only those hashes, then let
  // osx-sign seal the refreshed receipts with the top-level application signature.
  await signArtifactRuntimeMachO(options)
  await signOfficeCliMachO(options)
  await signOfficeRendererMachO(options)
  const osxSignModule = await import('@electron/osx-sign')
  // Never use the legacy `sign` export: it is callback-based, returns `undefined`, and would let
  // electron-builder continue to afterSign while codesign is still running. Only the Promise API
  // gives the packaging transaction a real completion boundary.
  const signApplication = resolvePromiseSigningFunction(osxSignModule)
  await signApplication(options)
}
