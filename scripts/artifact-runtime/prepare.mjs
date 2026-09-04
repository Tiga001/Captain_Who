/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { randomUUID } from 'node:crypto'
import { mkdir, rm } from 'node:fs/promises'
import { basename, dirname, join, resolve } from 'node:path'

import {
  DEFAULT_DOWNLOAD_DIRECTORY,
  DEFAULT_MANIFEST_PATH,
  DEFAULT_OUTPUT_DIRECTORY,
  loadArtifactRuntimeManifest,
  selectArtifactRuntimeAssets
} from './contract.mjs'
import { acquireNodeRuntime, acquireRipgrep, copyTreeRejectingSymlinks } from './archive.mjs'
import {
  prepareManagedNodeDependencies,
  verifyPreparedManagedNodeDependencies
} from './node-dependencies.mjs'
import {
  acquirePythonRuntime,
  copyRuntimeSupportFiles,
  installPythonDependencies,
  normalizePythonConsoleScriptShebangs,
  pruneManagedPythonTestFixtures
} from './python-runtime.mjs'
import {
  buildReceipt,
  probePreparedRuntimes,
  publishDirectoryAtomically,
  verifyReceipt,
  walkRegularFiles,
  writeReceipt
} from './receipt.mjs'
import { prepareArtifactRuntimeLegalEvidence } from './legal-evidence.mjs'

async function buildComponentSource({ manifestPath, manifest, staging, downloadDirectory }) {
  const {
    node: nodeAsset,
    python: pythonAsset,
    ripgrep: ripgrepAsset
  } = selectArtifactRuntimeAssets(manifest)
  const nodeExecutable = await acquireNodeRuntime(manifest, nodeAsset, staging, downloadDirectory)
  await prepareManagedNodeDependencies(manifest, staging)
  const pythonExecutable = await acquirePythonRuntime(
    manifest,
    pythonAsset,
    staging,
    downloadDirectory
  )
  await installPythonDependencies(manifest, pythonExecutable)
  await pruneManagedPythonTestFixtures(manifest, staging)
  await normalizePythonConsoleScriptShebangs(pythonExecutable)
  const ripgrepExecutable = await acquireRipgrep(manifest, ripgrepAsset, staging, downloadDirectory)
  await copyRuntimeSupportFiles(manifestPath, manifest, staging, downloadDirectory)
  await probePreparedRuntimes(
    manifest,
    staging,
    nodeExecutable,
    pythonExecutable,
    ripgrepExecutable
  )
}

export async function prepareArtifactRuntime({
  manifestPath = DEFAULT_MANIFEST_PATH,
  outputDirectory = DEFAULT_OUTPUT_DIRECTORY,
  downloadDirectory = DEFAULT_DOWNLOAD_DIRECTORY,
  sourceDirectory,
  platform = process.platform,
  arch = process.arch,
  verifyOnly = false,
  forceRebuild = false,
  hooks = {}
} = {}) {
  const manifest = await loadArtifactRuntimeManifest(manifestPath)
  selectArtifactRuntimeAssets(manifest, platform, arch)
  if (verifyOnly) {
    const receipt = await verifyReceipt(outputDirectory, manifest, platform, arch)
    return Object.freeze({ outputDirectory, receipt, reused: true })
  }
  if (!forceRebuild) {
    try {
      const receipt = await verifyReceipt(outputDirectory, manifest, platform, arch)
      return Object.freeze({ outputDirectory, receipt, reused: true })
    } catch {
      // Missing, stale, or damaged components are rebuilt from pinned inputs.
    }
  }

  await mkdir(dirname(outputDirectory), { recursive: true })
  const staging = join(
    dirname(outputDirectory),
    `.${basename(outputDirectory)}.${process.pid}.${randomUUID()}.staging`
  )
  await mkdir(staging, { recursive: false, mode: 0o700 })
  try {
    if (sourceDirectory) {
      const source = resolve(sourceDirectory)
      if (source === resolve(staging) || source === resolve(outputDirectory)) {
        throw new Error('Artifact runtime source directory cannot alias the staging directory')
      }
      await copyTreeRejectingSymlinks(source, staging, { destinationExists: true })
      await verifyPreparedManagedNodeDependencies(manifest, staging)
      await pruneManagedPythonTestFixtures(manifest, staging)
      const nodeExecutableRelative =
        platform === 'win32' ? manifest.node.executable.win32 : manifest.node.executable.unix
      const pythonExecutableRelative =
        platform === 'win32' ? manifest.python.executable.win32 : manifest.python.executable.unix
      const ripgrepExecutableRelative =
        platform === 'win32'
          ? manifest.tools.ripgrep.executable.win32
          : manifest.tools.ripgrep.executable.unix
      await probePreparedRuntimes(
        manifest,
        staging,
        join(staging, ...nodeExecutableRelative.split('/')),
        join(staging, ...pythonExecutableRelative.split('/')),
        join(staging, ...ripgrepExecutableRelative.split('/'))
      )
    } else {
      await buildComponentSource({ manifestPath, manifest, staging, downloadDirectory })
    }
    await prepareArtifactRuntimeLegalEvidence(manifest, staging)
    const files = await walkRegularFiles(staging)
    const receipt = buildReceipt(manifest, platform, arch, files)
    await writeReceipt(staging, receipt)
    await verifyReceipt(staging, manifest, platform, arch)
    await hooks.afterStaging?.({ staging, receipt })
    await publishDirectoryAtomically(staging, outputDirectory, hooks)
    const published = await verifyReceipt(outputDirectory, manifest, platform, arch)
    return Object.freeze({ outputDirectory, receipt: published, reused: false })
  } finally {
    await rm(staging, { recursive: true, force: true }).catch(() => undefined)
  }
}
