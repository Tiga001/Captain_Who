/* eslint-disable @typescript-eslint/explicit-function-return-type -- electron-builder loads this JavaScript module directly. */

import { readFile } from 'node:fs/promises'
import { basename, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import {
  afterPack as verifyOfficeRendererAfterPack,
  afterSign as verifyOfficeRendererAfterSign
} from './verify-packaged-office-renderer.mjs'
import { verifyPackagedWordPdfRenderer } from './verify-packaged-word-pdf-renderer.mjs'
import { verifyPackagedMacSignatures } from './verify-packaged-macos-signatures.mjs'

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)))

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
    throw new Error('Packaged MyCopilot icon does not match build/icon.icns')
  }
}

export function isMacCodeSigningExplicitlyDisabled(context) {
  return context?.packager?.platformSpecificBuildOptions?.identity === null
}

export async function afterPack(context) {
  await verifyOfficeRendererAfterPack(context)
  if (context.electronPlatformName === 'darwin') {
    await verifyPackagedWordPdfRenderer(context)
    await verifyPackagedMacIcon(context)
  }
}

export async function afterSign(context) {
  await verifyOfficeRendererAfterSign(context)
  if (context.electronPlatformName === 'darwin' && !isMacCodeSigningExplicitlyDisabled(context)) {
    await verifyPackagedWordPdfRenderer(context)
    await verifyPackagedMacSignatures(context)
  }
}
