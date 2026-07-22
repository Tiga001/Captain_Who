/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { basename, join } from 'node:path'

import { prepareOfficeRenderer } from './prepare-office-renderer.mjs'

const ELECTRON_BUILDER_ARCHITECTURES = new Map([
  [1, 'x64'],
  [3, 'arm64']
])

export function packagedOfficeRendererTargetPlatform(context, hostPlatform = process.platform) {
  if (!context || typeof context !== 'object') {
    throw new Error('electron-builder pack context is required')
  }
  const targetPlatform =
    context.electronPlatformName === 'mas' ? 'darwin' : context.electronPlatformName
  if (!['darwin', 'linux', 'win32'].includes(targetPlatform)) {
    throw new Error(
      `Unsupported electron-builder target platform: ${String(context.electronPlatformName)}`
    )
  }
  if (!['darwin', 'linux', 'win32'].includes(hostPlatform)) {
    throw new Error(`Unsupported Office renderer host platform: ${hostPlatform}`)
  }
  if (targetPlatform !== hostPlatform) {
    throw new Error(
      `Cross-platform Office renderer packaging is not supported: target ${targetPlatform}, host ${hostPlatform}`
    )
  }
  return targetPlatform
}

export function packagedOfficeRendererTargetArch(context, hostArch = process.arch) {
  if (!context || typeof context !== 'object') {
    throw new Error('electron-builder pack context is required')
  }
  const targetArch = ELECTRON_BUILDER_ARCHITECTURES.get(context.arch)
  if (targetArch === undefined) {
    throw new Error(`Unsupported electron-builder target architecture: ${String(context.arch)}`)
  }
  if (hostArch !== 'arm64' && hostArch !== 'x64') {
    throw new Error(`Unsupported Office renderer host architecture: ${hostArch}`)
  }
  if (targetArch !== hostArch) {
    throw new Error(
      `Cross-architecture Office renderer packaging is not supported: target ${targetArch}, host ${hostArch}`
    )
  }
  return targetArch
}

export function packagedOfficeRendererDirectory(context) {
  if (!context || typeof context !== 'object') {
    throw new Error('electron-builder pack context is required')
  }
  if (typeof context.appOutDir !== 'string' || context.appOutDir.length === 0) {
    throw new Error('electron-builder appOutDir is required')
  }
  const isMac = context.electronPlatformName === 'darwin' || context.electronPlatformName === 'mas'
  let resourcesDirectory
  if (isMac) {
    const productFilename = context.packager?.appInfo?.productFilename
    if (
      typeof productFilename !== 'string' ||
      productFilename.length === 0 ||
      basename(productFilename) !== productFilename
    ) {
      throw new Error('electron-builder productFilename is required for a macOS package')
    }
    resourcesDirectory = join(context.appOutDir, `${productFilename}.app`, 'Contents', 'Resources')
  } else {
    resourcesDirectory = join(context.appOutDir, 'resources')
  }
  return join(resourcesDirectory, 'components', 'office-renderer')
}

async function verifyPackagedOfficeRenderer(context) {
  const outputDirectory = packagedOfficeRendererDirectory(context)
  const arch = packagedOfficeRendererTargetArch(context)
  const platform = packagedOfficeRendererTargetPlatform(context)
  const result = await prepareOfficeRenderer({
    outputDirectory,
    verifyOnly: true,
    platform,
    arch
  })
  console.log(
    `Verified packaged Office renderer ${result.receipt.browser.version} ` +
      `(${result.receipt.bundleRevision}) at ${outputDirectory}`
  )
}

export async function afterPack(context) {
  await verifyPackagedOfficeRenderer(context)
}

export async function afterSign(context) {
  await verifyPackagedOfficeRenderer(context)
}
