/* eslint-disable @typescript-eslint/explicit-function-return-type -- electron-builder loads this JavaScript module directly. */

import { lstat, readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { basename, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import {
  afterPack as verifyOfficeRendererAfterPack,
  afterSign as verifyOfficeRendererAfterSign
} from './verify-packaged-office-renderer.mjs'
import {
  verifyPackagedFrozenComponentsAfterPack,
  verifyPackagedFrozenComponentsAfterSign
} from './verify-packaged-frozen-components.mjs'
import { verifyPackagedWordPdfRenderer } from './verify-packaged-word-pdf-renderer.mjs'
import { verifyPackagedMacSignatures } from './verify-packaged-macos-signatures.mjs'
import { sanitizePackagedMacNativeCode } from './sanitize-packaged-mac-native-code.mjs'
import { verifyPackagedPrivacy } from './verify-packaged-privacy.mjs'

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)))
const requireFromBuilder = createRequire(import.meta.resolve('electron-builder/package.json'))
const packagedRuntimeRequirements = new Map([
  ['node_modules/@playwright/mcp/package.json', ['@playwright/mcp', '0.0.79']],
  ['node_modules/@modelcontextprotocol/sdk/package.json', ['@modelcontextprotocol/sdk', '1.29.0']],
  ['node_modules/playwright/package.json', ['playwright', '1.63.0-alpha-2026-08-05']],
  ['node_modules/playwright-core/package.json', ['playwright-core', '1.63.0-alpha-2026-08-05']]
])

function packagedResourcesDirectory(context) {
  if (!context || typeof context !== 'object') {
    throw new Error('electron-builder pack context is required')
  }
  if (typeof context.appOutDir !== 'string' || context.appOutDir.length === 0) {
    throw new Error('electron-builder appOutDir is required')
  }
  const isMac = context.electronPlatformName === 'darwin' || context.electronPlatformName === 'mas'
  if (!isMac) return join(context.appOutDir, 'resources')

  const productFilename = context.packager?.appInfo?.productFilename
  if (
    typeof productFilename !== 'string' ||
    productFilename.length === 0 ||
    basename(productFilename) !== productFilename
  ) {
    throw new Error('electron-builder productFilename is required for a macOS package')
  }
  return join(context.appOutDir, `${productFilename}.app`, 'Contents', 'Resources')
}

export function packagedApplicationAsarPath(context) {
  return join(packagedResourcesDirectory(context), 'app.asar')
}

export async function verifyPackagedAutoUpdateMetadataDisabled(context) {
  // electron-builder 26 needs an explicit null to suppress repository inference and update
  // manifests. Target-level switches below independently suppress differential artifacts.
  const configuration = context?.packager?.config
  if (configuration?.publish !== null) {
    throw new Error('electron-builder publish must remain explicitly null while auto-update is off')
  }

  for (const scope of ['mac', 'win', 'linux', 'dmg', 'nsis', 'appImage']) {
    const scopedPublish = configuration?.[scope]?.publish
    if (scopedPublish !== undefined && scopedPublish !== null) {
      throw new Error(
        `electron-builder ${scope}.publish must remain unset while auto-update is off`
      )
    }
  }

  if (
    !Array.isArray(configuration?.mac?.target) ||
    configuration.mac.target.length !== 1 ||
    configuration.mac.target[0] !== 'dmg'
  ) {
    throw new Error('electron-builder mac.target must remain DMG-only while auto-update is off')
  }
  if (configuration?.dmg?.writeUpdateInfo !== false) {
    throw new Error(
      'electron-builder dmg.writeUpdateInfo must remain false while auto-update is off'
    )
  }
  if (configuration?.nsis?.differentialPackage !== false) {
    throw new Error(
      'electron-builder nsis.differentialPackage must remain false while auto-update is off'
    )
  }

  const updateConfiguration = join(packagedResourcesDirectory(context), 'app-update.yml')
  try {
    await lstat(updateConfiguration)
  } catch (error) {
    if (error?.code === 'ENOENT') return
    throw error
  }
  throw new Error('Packaged application unexpectedly contains app-update.yml')
}

function loadAsarApi() {
  // electron-builder owns this exact packaging dependency. Resolve from its dependency graph so
  // the application does not acquire a second ASAR implementation merely for an afterPack check.
  return requireFromBuilder('@electron/asar')
}

export async function verifyPackagedManagedPlaywrightMcp(context, asarApi = loadAsarApi()) {
  const asarPath = packagedApplicationAsarPath(context)
  const entries = new Set(
    asarApi.listPackage(asarPath).map((entry) => entry.replace(/^[/\\]+/, '').replaceAll('\\', '/'))
  )

  for (const [packagePath, [expectedName, expectedVersion]] of packagedRuntimeRequirements) {
    if (!entries.has(packagePath)) {
      throw new Error(`Packaged managed Playwright MCP is missing ${packagePath}`)
    }
    let manifest
    try {
      manifest = JSON.parse(asarApi.extractFile(asarPath, packagePath).toString('utf8'))
    } catch {
      throw new Error(`Packaged managed Playwright MCP has an unreadable ${packagePath}`)
    }
    if (manifest.name !== expectedName || manifest.version !== expectedVersion) {
      throw new Error(
        `Packaged managed Playwright MCP expected ${expectedName}@${expectedVersion} at ${packagePath}`
      )
    }
  }

  for (const entry of [
    'node_modules/@playwright/mcp/index.js',
    'node_modules/@modelcontextprotocol/sdk/dist/esm/client/index.js',
    'node_modules/playwright/lib/index.js',
    'node_modules/playwright-core/index.js'
  ]) {
    if (!entries.has(entry)) {
      throw new Error(`Packaged managed Playwright MCP entry is missing ${entry}`)
    }
  }

  const mcpManifest = JSON.parse(
    asarApi.extractFile(asarPath, 'node_modules/@playwright/mcp/package.json').toString('utf8')
  )
  if (
    mcpManifest.dependencies?.playwright !== '1.63.0-alpha-2026-08-05' ||
    mcpManifest.dependencies?.['playwright-core'] !== '1.63.0-alpha-2026-08-05'
  ) {
    throw new Error('Packaged @playwright/mcp dependency identity drifted')
  }
}

export function packagedMacIconPath(context) {
  if (!context || typeof context !== 'object' || context.electronPlatformName !== 'darwin') {
    throw new Error('A macOS electron-builder pack context is required')
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
  return join(context.appOutDir, `${productFilename}.app`, 'Contents', 'Resources', 'icon.icns')
}

export async function verifyPackagedMacIcon(
  context,
  sourceIconPath = join(repositoryRoot, 'build', 'icon.icns')
) {
  const [sourceIcon, packagedIcon] = await Promise.all([
    readFile(sourceIconPath),
    readFile(packagedMacIconPath(context))
  ])
  if (!packagedIcon.equals(sourceIcon)) {
    throw new Error('Packaged Captain Who icon does not match build/icon.icns')
  }
}

export function isMacCodeSigningExplicitlyDisabled(context) {
  return context?.packager?.platformSpecificBuildOptions?.identity === null
}

export async function afterPack(context) {
  await verifyPackagedAutoUpdateMetadataDisabled(context)
  if (context.electronPlatformName === 'darwin') {
    await sanitizePackagedMacNativeCode(context)
    await verifyPackagedPrivacy(context)
  }
  await verifyPackagedFrozenComponentsAfterPack(context)
  await verifyOfficeRendererAfterPack(context)
  await verifyPackagedWordPdfRenderer(context)
  await verifyPackagedManagedPlaywrightMcp(context)
  if (context.electronPlatformName === 'darwin') {
    await verifyPackagedMacIcon(context)
  }
}

export async function afterSign(context) {
  await verifyPackagedAutoUpdateMetadataDisabled(context)
  const frozenComponents = await verifyPackagedFrozenComponentsAfterSign(context)
  const officeRenderer = await verifyOfficeRendererAfterSign(context)
  if (context.electronPlatformName === 'darwin' && !isMacCodeSigningExplicitlyDisabled(context)) {
    await verifyPackagedWordPdfRenderer(context)
    await verifyPackagedMacSignatures(context, undefined, {
      officeRendererReceipt: officeRenderer.receipt,
      frozenComponents
    })
  } else if (context.electronPlatformName !== 'darwin') {
    await verifyPackagedWordPdfRenderer(context)
  }
}
