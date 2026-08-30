/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { execFile as execFileCallback } from 'node:child_process'
import { basename, join } from 'node:path'
import { promisify } from 'node:util'

import {
  CORE_SERVER_CODE_SIGN_IDENTIFIER,
  artifactRuntimeMacCodeSigningTargets,
  officeCliMacCodeSigningTargets,
  officeRendererMacCodeSigningTargets
} from './sign-macos.mjs'
import { collectFrozenMachOTargets } from './frozen-macho-signing.mjs'

const execFile = promisify(execFileCallback)
const APP_CODE_SIGN_IDENTIFIER = 'com.mycopilot.next'
export function packagedMacApplicationPaths(context) {
  if (!context || typeof context !== 'object') {
    throw new Error('electron-builder pack context is required')
  }
  if (context.electronPlatformName !== 'darwin') {
    throw new Error(
      `macOS signature verification requires a darwin package, got ${String(context.electronPlatformName)}`
    )
  }
  if (typeof context.appOutDir !== 'string' || context.appOutDir.length === 0) {
    throw new Error('electron-builder appOutDir is required')
  }
  const productFilename = context.packager?.appInfo?.productFilename
  if (
    typeof productFilename !== 'string' ||
    productFilename.length === 0 ||
    basename(productFilename) !== productFilename
  ) {
    throw new Error('electron-builder productFilename is required for a macOS package')
  }

  const app = join(context.appOutDir, `${productFilename}.app`)
  return {
    app,
    coreServer: join(app, 'Contents', 'Resources', 'core-server')
  }
}

export function parseCodeSignatureMetadata(output) {
  if (typeof output !== 'string') {
    throw new Error('codesign metadata output must be a string')
  }
  const field = (name) => {
    const match = output.match(new RegExp(`^${name}=(.+)$`, 'm'))
    return match?.[1].trim()
  }
  return {
    identifier: field('Identifier'),
    teamIdentifier: field('TeamIdentifier'),
    authorities: [...output.matchAll(/^Authority=(.+)$/gm)].map((match) => match[1].trim()),
    adHoc: /^Signature=adhoc$/m.test(output),
    runtime: /^CodeDirectory .*\bflags=.*\bruntime\b/m.test(output),
    timestamp: field('Timestamp')
  }
}

function parseBooleanEntitlements(output, label) {
  if (typeof output !== 'string') {
    throw new Error(`${label} entitlements must be XML text`)
  }
  const keys = [...output.matchAll(/<key>\s*([^<]+)\s*<\/key>/g)].map((match) => match[1].trim())
  const pairs = [...output.matchAll(/<key>\s*([^<]+)\s*<\/key>\s*<(true|false)\s*\/>/g)].map(
    (match) => [match[1].trim(), match[2] === 'true']
  )
  if (pairs.length !== keys.length || new Set(keys).size !== keys.length) {
    throw new Error(`${label} entitlements are malformed or non-boolean`)
  }
  return new Map(pairs)
}

export function assertStableSignatureMetadata({
  appMetadata,
  helperMetadata,
  officeRendererMetadata = [],
  frozenComponentMetadata = [],
  designatedRequirement,
  helperEntitlements
}) {
  if (appMetadata.identifier !== APP_CODE_SIGN_IDENTIFIER) {
    throw new Error(
      `MyCopilot application has unexpected code-sign identifier: ${String(appMetadata.identifier)}`
    )
  }
  if (helperMetadata.identifier !== CORE_SERVER_CODE_SIGN_IDENTIFIER) {
    throw new Error(
      `core-server has unexpected code-sign identifier: ${String(helperMetadata.identifier)}`
    )
  }
  for (const [label, metadata] of [
    ['MyCopilot application', appMetadata],
    ['core-server', helperMetadata]
  ]) {
    if (
      metadata.adHoc ||
      metadata.teamIdentifier === undefined ||
      metadata.teamIdentifier === 'not set'
    ) {
      throw new Error(`${label} is unsigned or ad-hoc signed`)
    }
    if (
      !metadata.authorities.some((authority) => authority.startsWith('Developer ID Application:'))
    ) {
      throw new Error(`${label} is not signed by a Developer ID Application identity`)
    }
    if (!metadata.runtime) {
      throw new Error(`${label} is not signed with the hardened runtime`)
    }
    if (metadata.timestamp === undefined || metadata.timestamp === 'none') {
      throw new Error(`${label} does not have a trusted signing timestamp`)
    }
  }
  if (appMetadata.teamIdentifier !== helperMetadata.teamIdentifier) {
    throw new Error(
      `MyCopilot and core-server Team IDs differ: ${appMetadata.teamIdentifier} vs ${helperMetadata.teamIdentifier}`
    )
  }
  if (appMetadata.authorities[0] !== helperMetadata.authorities[0]) {
    throw new Error('MyCopilot and core-server are not signed by the same leaf identity')
  }
  for (const { target, metadata } of officeRendererMetadata) {
    if (metadata.identifier !== target.identifier) {
      throw new Error(
        `Office renderer has an unexpected code-sign identifier: ${target.relativePath}`
      )
    }
    if (
      metadata.adHoc ||
      metadata.teamIdentifier === undefined ||
      metadata.teamIdentifier === 'not set' ||
      !metadata.authorities.some((authority) => authority.startsWith('Developer ID Application:'))
    ) {
      throw new Error(`Office renderer is unsigned or ad-hoc signed: ${target.relativePath}`)
    }
    if (!metadata.runtime || metadata.timestamp === undefined || metadata.timestamp === 'none') {
      throw new Error(`Office renderer lacks hardened runtime or timestamp: ${target.relativePath}`)
    }
    if (
      metadata.teamIdentifier !== appMetadata.teamIdentifier ||
      metadata.authorities[0] !== appMetadata.authorities[0]
    ) {
      throw new Error(
        'MyCopilot and Office renderer are not signed by the same Developer ID identity'
      )
    }
  }
  for (const {
    component,
    target,
    metadata,
    entitlements,
    expectedEntitlements = []
  } of frozenComponentMetadata) {
    if (metadata.identifier !== target.identifier) {
      throw new Error(`${component} has an unexpected code-sign identifier: ${target.relativePath}`)
    }
    if (
      metadata.adHoc ||
      metadata.teamIdentifier === undefined ||
      metadata.teamIdentifier === 'not set' ||
      !metadata.authorities.some((authority) => authority.startsWith('Developer ID Application:'))
    ) {
      throw new Error(`${component} is unsigned or ad-hoc signed: ${target.relativePath}`)
    }
    if (!metadata.runtime || metadata.timestamp === undefined || metadata.timestamp === 'none') {
      throw new Error(`${component} lacks hardened runtime or timestamp: ${target.relativePath}`)
    }
    if (
      metadata.teamIdentifier !== appMetadata.teamIdentifier ||
      metadata.authorities[0] !== appMetadata.authorities[0]
    ) {
      throw new Error(`${component} and MyCopilot are not signed by the same Developer ID identity`)
    }
    const entitlementMap = parseBooleanEntitlements(entitlements, component)
    const entitlementKeys = [...entitlementMap.keys()].sort()
    const expectedKeys = [...expectedEntitlements].sort()
    if (
      entitlementKeys.length !== expectedKeys.length ||
      entitlementKeys.some(
        (key, index) => key !== expectedKeys[index] || entitlementMap.get(key) !== true
      )
    ) {
      throw new Error(`${component} has unexpected entitlements: ${target.relativePath}`)
    }
  }
  if (/\bcdhash\b/i.test(designatedRequirement)) {
    throw new Error('core-server designated requirement is tied to a mutable cdhash')
  }
  if (
    !designatedRequirement.includes(`identifier "${CORE_SERVER_CODE_SIGN_IDENTIFIER}"`) ||
    !designatedRequirement.includes('anchor apple generic') ||
    !designatedRequirement.includes('certificate leaf')
  ) {
    throw new Error('core-server designated requirement is not bound to its signer and identifier')
  }
  const helperEntitlementKeys = [
    ...parseBooleanEntitlements(helperEntitlements, 'core-server').keys()
  ]
  if (helperEntitlementKeys.length > 0) {
    throw new Error(
      `core-server contains unexpected entitlement(s): ${helperEntitlementKeys.join(', ')}`
    )
  }
}

async function runCodesign(argumentsList, run) {
  const result = await run('/usr/bin/codesign', argumentsList, {
    encoding: 'utf8',
    maxBuffer: 1024 * 1024
  })
  return `${result.stdout ?? ''}\n${result.stderr ?? ''}`
}

async function frozenComponentSigningTargets(frozenComponents) {
  if (!frozenComponents || typeof frozenComponents !== 'object') {
    throw new Error('Packaged frozen component verification result is required')
  }
  const { directories, artifactRuntime, officeCli } = frozenComponents
  if (
    typeof directories?.artifactRuntime !== 'string' ||
    typeof directories?.officeCli !== 'string' ||
    !Array.isArray(artifactRuntime?.receipt?.files) ||
    !Array.isArray(officeCli?.receipt?.files)
  ) {
    throw new Error('Packaged frozen component verification result is incomplete')
  }
  const artifactTargets = artifactRuntimeMacCodeSigningTargets(
    await collectFrozenMachOTargets({
      outputDirectory: directories.artifactRuntime,
      files: artifactRuntime.receipt.files,
      pathKey: 'path',
      label: 'Artifact Runtime'
    })
  )
  const officeCliTargets = officeCliMacCodeSigningTargets(
    await collectFrozenMachOTargets({
      outputDirectory: directories.officeCli,
      files: officeCli.receipt.files,
      pathKey: 'name',
      label: 'OfficeCLI'
    })
  )
  return [
    ...artifactTargets.map((target) => ({ component: 'Artifact Runtime', target })),
    ...officeCliTargets.map((target) => ({ component: 'OfficeCLI', target }))
  ]
}

export async function verifyPackagedMacSignatures(
  context,
  run = execFile,
  verification,
  { resolveFrozenTargets = frozenComponentSigningTargets } = {}
) {
  const paths = packagedMacApplicationPaths(context)
  if (!verification || typeof verification !== 'object') {
    throw new Error('Packaged macOS signature verification inputs are required')
  }
  if (typeof resolveFrozenTargets !== 'function') {
    throw new Error('Packaged frozen target resolver must be a function')
  }
  const officeRendererTargets = officeRendererMacCodeSigningTargets(
    paths.app,
    verification.officeRendererReceipt
  )
  const frozenTargets = await resolveFrozenTargets(verification.frozenComponents)

  await runCodesign(['--verify', '--deep', '--strict', '--verbose=2', paths.app], run)
  await runCodesign(['--verify', '--strict', '--verbose=2', paths.coreServer], run)
  for (const target of officeRendererTargets) {
    await runCodesign(['--verify', '--strict', '--verbose=2', target.path], run)
  }
  for (const { target } of frozenTargets) {
    await runCodesign(['--verify', '--strict', '--verbose=2', target.path], run)
  }

  const appDetails = await runCodesign(['--display', '--verbose=4', paths.app], run)
  const helperDetails = await runCodesign(['--display', '--verbose=4', paths.coreServer], run)
  const officeRendererMetadata = []
  for (const target of officeRendererTargets) {
    const details = await runCodesign(['--display', '--verbose=4', target.path], run)
    officeRendererMetadata.push({ target, metadata: parseCodeSignatureMetadata(details) })
  }
  const frozenComponentMetadata = []
  for (const { component, target } of frozenTargets) {
    const details = await runCodesign(['--display', '--verbose=4', target.path], run)
    const entitlements = await runCodesign(
      ['--display', '--entitlements', '-', '--xml', target.path],
      run
    )
    frozenComponentMetadata.push({
      component,
      target,
      metadata: parseCodeSignatureMetadata(details),
      entitlements,
      expectedEntitlements:
        target.relativePath === 'dependencies/node/bin/node' || component === 'OfficeCLI'
          ? ['com.apple.security.cs.allow-jit']
          : []
    })
  }
  const designatedRequirement = await runCodesign(['--display', '-r-', paths.coreServer], run)
  const helperEntitlements = await runCodesign(
    ['--display', '--entitlements', '-', '--xml', paths.coreServer],
    run
  )

  const appMetadata = parseCodeSignatureMetadata(appDetails)
  const helperMetadata = parseCodeSignatureMetadata(helperDetails)
  assertStableSignatureMetadata({
    appMetadata,
    helperMetadata,
    officeRendererMetadata,
    frozenComponentMetadata,
    designatedRequirement,
    helperEntitlements
  })

  console.log(
    `Verified Developer ID signatures for MyCopilot and ${2 + officeRendererTargets.length + frozenTargets.length} managed native targets`
  )
}
