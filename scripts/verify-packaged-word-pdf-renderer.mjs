/* eslint-disable @typescript-eslint/explicit-function-return-type -- electron-builder loads this module directly. */

import { basename, join } from 'node:path'

import {
  loadWordPdfRendererManifest,
  prepareWordPdfRenderer,
  selectWordPdfRendererTarget,
  verifyWordPdfRendererCodeSignature
} from './prepare-word-pdf-renderer.mjs'

const ARCHITECTURES = new Map([
  [1, 'x64'],
  [3, 'arm64']
])

export function packagedWordPdfRendererDirectory(context) {
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
    throw new Error('electron-builder productFilename is required')
  }
  return join(
    context.appOutDir,
    `${productFilename}.app`,
    'Contents',
    'Resources',
    'components',
    'word-pdf-renderer'
  )
}

export function packagedWordPdfRendererTargetArch(context, hostArch = process.arch) {
  const arch = ARCHITECTURES.get(context?.arch)
  if (!arch || (arch !== 'arm64' && arch !== 'x64')) {
    throw new Error(`Unsupported electron-builder target architecture: ${String(context?.arch)}`)
  }
  if (arch !== hostArch) {
    throw new Error(`Cross-architecture Word PDF renderer packaging is not supported: ${arch}`)
  }
  return arch
}

export async function verifyPackagedWordPdfRenderer(context, hostArch = process.arch) {
  if (process.platform !== 'darwin') {
    throw new Error('Word PDF renderer packaging is currently supported only on macOS')
  }
  const arch = packagedWordPdfRendererTargetArch(context, hostArch)
  const outputDirectory = packagedWordPdfRendererDirectory(context)
  const result = await prepareWordPdfRenderer({
    outputDirectory,
    verifyOnly: true,
    platform: 'darwin',
    arch
  })
  const manifest = await loadWordPdfRendererManifest()
  const target = selectWordPdfRendererTarget(manifest, 'darwin', arch)
  await verifyWordPdfRendererCodeSignature(outputDirectory, target)
  console.log(
    `Verified packaged Word PDF renderer ${result.receipt.runtime.version} ` +
      `(${result.receipt.bundleRevision}) at ${outputDirectory}`
  )
}
