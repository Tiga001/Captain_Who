/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { execFile as execFileCallback } from 'node:child_process'
import { basename, join } from 'node:path'
import { promisify } from 'node:util'

import { CORE_SERVER_CODE_SIGN_IDENTIFIER } from './sign-macos.mjs'

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

export function assertStableSignatureMetadata({
  appMetadata,
  helperMetadata,
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
  const helperEntitlementKeys = [...helperEntitlements.matchAll(/<key>\s*([^<]+)\s*<\/key>/g)].map(
    (match) => match[1].trim()
  )
  if (helperEntitlementKeys.length > 0) {
    throw new Error(
      `core-server contains unexpected entitlement(s): ${helperEntitlementKeys.join(', ')}`
    )
  }
}

async function runCodesign(argumentsList, run) {
  const result = await run('codesign', argumentsList, {
    encoding: 'utf8',
    maxBuffer: 1024 * 1024
  })
  return `${result.stdout ?? ''}\n${result.stderr ?? ''}`
}

export async function verifyPackagedMacSignatures(context, run = execFile) {
  const paths = packagedMacApplicationPaths(context)

  await runCodesign(['--verify', '--deep', '--strict', '--verbose=2', paths.app], run)
  await runCodesign(['--verify', '--strict', '--verbose=2', paths.coreServer], run)

  const appDetails = await runCodesign(['--display', '--verbose=4', paths.app], run)
  const helperDetails = await runCodesign(['--display', '--verbose=4', paths.coreServer], run)
  const designatedRequirement = await runCodesign(['--display', '-r-', paths.coreServer], run)
  const helperEntitlements = await runCodesign(
    ['--display', '--entitlements', '-', paths.coreServer],
    run
  )

  const appMetadata = parseCodeSignatureMetadata(appDetails)
  const helperMetadata = parseCodeSignatureMetadata(helperDetails)
  assertStableSignatureMetadata({
    appMetadata,
    helperMetadata,
    designatedRequirement,
    helperEntitlements
  })

  console.log(
    `Verified Developer ID signatures for MyCopilot and core-server ` +
      `(Team ID ${appMetadata.teamIdentifier})`
  )
}
