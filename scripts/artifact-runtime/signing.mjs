/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { lstat, readFile } from 'node:fs/promises'
import { join } from 'node:path'
import {
  assertExactMachOSigningPaths,
  assertOnlyFrozenMachOFilesChanged,
  collectFrozenMachOTargets
} from '../frozen-macho-signing.mjs'

import {
  DEFAULT_MANIFEST_PATH,
  DEFAULT_OUTPUT_DIRECTORY,
  RECEIPT_NAME,
  loadArtifactRuntimeManifest,
  selectArtifactRuntimeAssets
} from './contract.mjs'
import { prepareArtifactRuntime } from './prepare.mjs'
import {
  buildReceipt,
  replaceReceiptAtomically,
  validateFrozenArtifactRuntimeReceipt,
  verifyReceipt,
  walkRegularFiles
} from './receipt.mjs'

export async function prepareArtifactRuntimeMacSigning({
  manifestPath = DEFAULT_MANIFEST_PATH,
  outputDirectory = DEFAULT_OUTPUT_DIRECTORY,
  platform = 'darwin',
  arch = process.arch
} = {}) {
  if (platform !== 'darwin') {
    throw new Error(`Artifact runtime macOS signing requires darwin, got ${platform}`)
  }
  const prepared = await prepareArtifactRuntime({
    manifestPath,
    outputDirectory,
    platform,
    arch,
    verifyOnly: true
  })
  const targets = await collectFrozenMachOTargets({
    outputDirectory,
    files: prepared.receipt.files,
    pathKey: 'path',
    label: 'Artifact runtime'
  })
  if (targets.length === 0) {
    throw new Error('Artifact runtime does not contain any frozen Mach-O files')
  }
  return Object.freeze({ outputDirectory, receipt: prepared.receipt, targets })
}

export async function refreshArtifactRuntimeReceiptAfterSigning({
  outputDirectory,
  originalReceipt,
  signedPaths,
  manifestPath = DEFAULT_MANIFEST_PATH,
  platform = 'darwin',
  arch = process.arch
}) {
  if (platform !== 'darwin') {
    throw new Error(`Artifact runtime macOS signing requires darwin, got ${platform}`)
  }
  const manifest = await loadArtifactRuntimeManifest(manifestPath)
  selectArtifactRuntimeAssets(manifest, platform, arch)
  const frozen = validateFrozenArtifactRuntimeReceipt(originalReceipt, manifest, platform, arch)

  const receiptPath = join(outputDirectory, RECEIPT_NAME)
  const receiptMetadata = await lstat(receiptPath)
  if (!receiptMetadata.isFile() || receiptMetadata.isSymbolicLink()) {
    throw new Error(
      'Artifact runtime receipt must remain a regular non-symlink file during signing'
    )
  }
  const currentReceipt = JSON.parse(await readFile(receiptPath, 'utf8'))
  if (JSON.stringify(currentReceipt) !== JSON.stringify(frozen)) {
    throw new Error('Artifact runtime receipt changed during the signing transaction')
  }

  const actualFiles = await walkRegularFiles(outputDirectory)
  const targets = await collectFrozenMachOTargets({
    outputDirectory,
    files: actualFiles,
    pathKey: 'path',
    label: 'Artifact runtime'
  })
  const canonicalSignedPaths = assertExactMachOSigningPaths({
    signedPaths,
    targets,
    label: 'Artifact runtime'
  })
  assertOnlyFrozenMachOFilesChanged({
    beforeFiles: frozen.files,
    afterFiles: actualFiles,
    signedPaths: canonicalSignedPaths,
    pathKey: 'path',
    label: 'Artifact runtime'
  })

  const refreshed = buildReceipt(manifest, platform, arch, actualFiles)
  validateFrozenArtifactRuntimeReceipt(refreshed, manifest, platform, arch)
  await replaceReceiptAtomically(outputDirectory, refreshed)
  return verifyReceipt(outputDirectory, manifest, platform, arch)
}
