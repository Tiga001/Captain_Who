/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

import { prepareArtifactRuntime } from './artifact-runtime/prepare.mjs'

export {
  ARTIFACT_RUNTIME_MAX_DOWNLOAD_BYTES,
  ARTIFACT_RUNTIME_MAX_FILES,
  ARTIFACT_RUNTIME_MAX_REDIRECTS,
  ARTIFACT_RUNTIME_MAX_TOTAL_BYTES,
  loadArtifactRuntimeManifest,
  selectArtifactRuntimeAssets,
  validateArtifactRuntimeDownloadUrl,
  validateArtifactRuntimeManifest,
  verifyArtifactRuntimeBuildInputs
} from './artifact-runtime/contract.mjs'
export { readPinnedZipMembers } from './artifact-runtime/archive.mjs'
export {
  createManagedNodeDependencyEvidence,
  parsePnpmLockPackageIntegrities,
  prepareManagedNodeDependencies
} from './artifact-runtime/node-dependencies.mjs'
export { normalizePythonConsoleScriptShebangs } from './artifact-runtime/python-runtime.mjs'
export { prepareArtifactRuntimeLegalEvidence } from './artifact-runtime/legal-evidence.mjs'
export {
  prepareArtifactRuntimeMacSigning,
  refreshArtifactRuntimeReceiptAfterSigning
} from './artifact-runtime/signing.mjs'
export { prepareArtifactRuntime }

function parseArguments(argv) {
  const options = {}
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === '--verify') {
      options.verifyOnly = true
    } else if (argument === '--force') {
      options.forceRebuild = true
    } else if (argument === '--output' || argument === '--source' || argument === '--downloads') {
      const value = argv[index + 1]
      if (!value) throw new Error(`${argument} requires a path`)
      index += 1
      if (argument === '--output') options.outputDirectory = resolve(value)
      if (argument === '--source') options.sourceDirectory = resolve(value)
      if (argument === '--downloads') options.downloadDirectory = resolve(value)
    } else {
      throw new Error(`Unknown argument: ${argument}`)
    }
  }
  return options
}

async function main() {
  const prepared = await prepareArtifactRuntime(parseArguments(process.argv.slice(2)))
  console.log(
    `${prepared.reused ? 'Verified' : 'Prepared'} managed Artifact Runtime ` +
      `${prepared.receipt.bundleVersion} (${prepared.receipt.bundleRevision}) at ${prepared.outputDirectory}`
  )
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined
if (invokedPath === import.meta.url) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  })
}
