/* eslint-disable @typescript-eslint/explicit-function-return-type -- electron-builder loads this JavaScript module directly. */

import { basename, join } from 'node:path'

import { prepareArtifactRuntime } from './prepare-artifact-runtime.mjs'
import { verifyPackagedOfficeCli } from './prepare-officecli.mjs'

const ELECTRON_BUILDER_ARCHITECTURES = new Map([
  [1, 'x64'],
  [3, 'arm64']
])

export function packagedFrozenComponentTargetPlatform(context, hostPlatform = process.platform) {
  if (!context || typeof context !== 'object') {
    throw new Error('electron-builder pack context is required')
  }
  const platform = context.electronPlatformName === 'mas' ? 'darwin' : context.electronPlatformName
  if (!['darwin', 'linux', 'win32'].includes(platform)) {
    throw new Error(
      `Unsupported electron-builder target platform: ${String(context.electronPlatformName)}`
    )
  }
  if (platform !== hostPlatform) {
    throw new Error(
      `Cross-platform frozen component packaging is not supported: target ${platform}, host ${hostPlatform}`
    )
  }
  return platform
}

export function packagedFrozenComponentTargetArch(context, hostArch = process.arch) {
  if (!context || typeof context !== 'object') {
    throw new Error('electron-builder pack context is required')
  }
  const arch = ELECTRON_BUILDER_ARCHITECTURES.get(context.arch)
  if (arch === undefined) {
    throw new Error(`Unsupported electron-builder target architecture: ${String(context.arch)}`)
  }
  if (arch !== hostArch) {
    throw new Error(
      `Cross-architecture frozen component packaging is not supported: target ${arch}, host ${hostArch}`
    )
  }
  return arch
}

export function packagedFrozenComponentDirectories(context) {
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
  return Object.freeze({
    artifactRuntime: join(resourcesDirectory, 'components', 'artifact-runtime'),
    officeCli: join(resourcesDirectory, 'components', 'officecli')
  })
}

export function isPackagedMacSigningExplicitlyDisabled(context) {
  return context?.packager?.platformSpecificBuildOptions?.identity === null
}

export async function verifyPackagedFrozenComponents(
  context,
  {
    signedOfficeCli,
    hostPlatform = process.platform,
    hostArch = process.arch,
    artifactVerifier = prepareArtifactRuntime,
    officeCliVerifier = verifyPackagedOfficeCli
  } = {}
) {
  if (typeof signedOfficeCli !== 'boolean') {
    throw new Error('Packaged OfficeCLI signed state must be explicit')
  }
  if (typeof artifactVerifier !== 'function' || typeof officeCliVerifier !== 'function') {
    throw new Error('Packaged frozen component verifiers must be functions')
  }
  const platform = packagedFrozenComponentTargetPlatform(context, hostPlatform)
  const arch = packagedFrozenComponentTargetArch(context, hostArch)
  if (signedOfficeCli && platform !== 'darwin') {
    throw new Error('Only a packaged macOS OfficeCLI component may use the signed receipt state')
  }
  const directories = packagedFrozenComponentDirectories(context)
  const artifactRuntime = await artifactVerifier({
    outputDirectory: directories.artifactRuntime,
    platform,
    arch,
    verifyOnly: true
  })
  const officeCli = await officeCliVerifier({
    outputDirectory: directories.officeCli,
    platform,
    arch,
    signed: signedOfficeCli
  })
  return Object.freeze({ directories, artifactRuntime, officeCli })
}

export async function verifyPackagedFrozenComponentsAfterPack(context, dependencies = {}) {
  return verifyPackagedFrozenComponents(context, { ...dependencies, signedOfficeCli: false })
}

export async function verifyPackagedFrozenComponentsAfterSign(context, dependencies = {}) {
  const platform =
    context?.electronPlatformName === 'mas' ? 'darwin' : context?.electronPlatformName
  const signedOfficeCli = platform === 'darwin' && !isPackagedMacSigningExplicitlyDisabled(context)
  return verifyPackagedFrozenComponents(context, { ...dependencies, signedOfficeCli })
}
