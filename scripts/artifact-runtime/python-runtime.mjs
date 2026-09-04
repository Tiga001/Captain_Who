/* eslint-disable @typescript-eslint/explicit-function-return-type -- Packaging boundary is runtime-validated JavaScript. */

import { randomUUID } from 'node:crypto'
import { chmod, cp, lstat, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { basename, dirname, join } from 'node:path'
import { extract } from 'tar'

import {
  archiveCachePath,
  copyWithoutSymlinks,
  downloadPinnedFile,
  runProcess
} from './archive.mjs'
import {
  BUILD_INPUT_SOURCE_PATHS,
  NODE_BOOTSTRAP_SOURCE,
  NODE_LOADER_SOURCE,
  PRESENTATION_SDK_SOURCE,
  REPOSITORY_ROOT
} from './contract.mjs'
import { verifyPinnedLocalFile } from './filesystem.mjs'

export async function acquirePythonRuntime(manifest, asset, staging, downloadDirectory) {
  const archive = await downloadPinnedFile(asset, archiveCachePath(downloadDirectory, asset))
  const extraction = join(staging, `.python-extract-${randomUUID()}`)
  await mkdir(extraction, { recursive: true })
  const destination = join(staging, ...manifest.python.runtimeHome.split('/'))
  try {
    await extract({ file: archive, cwd: extraction, strict: true, preservePaths: false })
    const source = join(extraction, 'python')
    await copyWithoutSymlinks(source, destination)
  } finally {
    await rm(extraction, { recursive: true, force: true })
  }
  const executableRelative =
    process.platform === 'win32'
      ? manifest.python.executable.win32
      : manifest.python.executable.unix
  const executable = join(staging, ...executableRelative.split('/'))
  if (process.platform !== 'win32') await chmod(executable, 0o755)
  const result = await runProcess(executable, ['--version'], { timeoutMs: 15_000 })
  const reported = `${result.stdout}\n${result.stderr}`.trim()
  if (!reported.includes(`Python ${manifest.python.version}`)) {
    throw new Error(`Managed Python probe returned unexpected version: ${reported}`)
  }
  return executable
}

export async function installPythonDependencies(manifest, pythonExecutable) {
  const requirements = join(REPOSITORY_ROOT, ...manifest.python.requirements.split('/'))
  await runProcess(
    pythonExecutable,
    [
      '-I',
      // Isolated mode ignores PYTHONDONTWRITEBYTECODE. Pass -B explicitly so pip cannot write
      // bytecode whose co_filename embeds this Mac's private staging path into the package.
      '-B',
      '-m',
      'pip',
      'install',
      '--isolated',
      '--require-hashes',
      '--only-binary=:all:',
      '--no-cache-dir',
      '--no-compile',
      '--no-warn-script-location',
      '--requirement',
      requirements
    ],
    { timeoutMs: 600_000 }
  )
}

export async function pruneManagedPythonTestFixtures(manifest, staging) {
  const pandas = manifest.python.dependencies.find((dependency) => dependency.name === 'pandas')
  if (!pandas) {
    throw new Error('Managed Python test pruning requires the pinned pandas dependency')
  }

  const metadataSuffix = `/pandas-${pandas.version}.dist-info/METADATA`
  if (!pandas.identityFile.endsWith(metadataSuffix)) {
    throw new Error('Managed pandas identity file has an unexpected package layout')
  }
  const sitePackagesRoot = pandas.identityFile.slice(0, -metadataSuffix.length)
  const testsRelative = `${sitePackagesRoot}/pandas/tests`
  const testsDirectory = join(staging, ...testsRelative.split('/'))
  let metadata
  try {
    metadata = await lstat(testsDirectory)
  } catch (error) {
    if (error?.code === 'ENOENT') {
      throw new Error('Pinned pandas wheel is missing its expected removable test fixture tree')
    }
    throw error
  }
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new Error('Pinned pandas test fixture tree must be a real directory')
  }

  // pandas wheels include their upstream test suite. It is not needed at runtime and contains
  // credential-shaped URL fixtures that must never be copied into the distributable application.
  await rm(testsDirectory, { recursive: true, force: false })
}

export async function normalizePythonConsoleScriptShebangs(pythonExecutable) {
  if (process.platform === 'win32') return Object.freeze([])

  const binDirectory = dirname(pythonExecutable)
  const expectedShebang = `#!${pythonExecutable}`
  const portableLauncher =
    `#!/bin/sh\n` +
    `'''exec' "$(dirname -- "$(realpath -- "$0")")/${basename(pythonExecutable)}" "$0" "$@"\n` +
    `' '''\n`
  const normalized = []

  for (const entry of await readdir(binDirectory, { withFileTypes: true })) {
    if (!entry.isFile()) continue
    const path = join(binDirectory, entry.name)
    const contents = await readFile(path)
    const firstNewline = contents.indexOf(0x0a)
    if (firstNewline === -1) continue
    const firstLine = contents.subarray(0, firstNewline).toString('utf8').replace(/\r$/, '')
    if (firstLine !== expectedShebang) continue

    await writeFile(
      path,
      Buffer.concat([Buffer.from(portableLauncher), contents.subarray(firstNewline + 1)])
    )
    normalized.push(entry.name)
  }

  return Object.freeze(normalized.sort())
}

export async function copyRuntimeSupportFiles(manifestPath, manifest, staging, downloadDirectory) {
  await cp(manifestPath, join(staging, 'runtime-manifest.json'), {
    force: false,
    errorOnExist: true
  })
  const runtimeDirectory = join(staging, 'runtime')
  await mkdir(runtimeDirectory, { recursive: true })
  await cp(NODE_BOOTSTRAP_SOURCE, join(staging, ...manifest.node.bootstrap.split('/')), {
    force: false,
    errorOnExist: true
  })
  await cp(NODE_LOADER_SOURCE, join(staging, ...manifest.node.loader.split('/')), {
    force: false,
    errorOnExist: true
  })
  await cp(PRESENTATION_SDK_SOURCE, join(staging, ...manifest.node.presentationSdk.split('/')), {
    force: false,
    errorOnExist: true
  })
  await verifyPinnedLocalFile(
    join(staging, ...manifest.node.bootstrap.split('/')),
    manifest.buildInputs.nodeBootstrap.sha256,
    'staged managed Node bootstrap'
  )
  await verifyPinnedLocalFile(
    join(staging, ...manifest.node.loader.split('/')),
    manifest.buildInputs.nodeLoader.sha256,
    'staged managed Node loader'
  )
  await verifyPinnedLocalFile(
    join(staging, ...manifest.node.presentationSdk.split('/')),
    manifest.buildInputs.presentationSdk.sha256,
    'staged Presentation Editor SDK'
  )
  const pdfCliSource = BUILD_INPUT_SOURCE_PATHS.pdfRuntimeCli
  const pdfCliTarget = join(staging, ...manifest.tools.pdfCli.target.split('/'))
  await cp(pdfCliSource, pdfCliTarget, { force: false, errorOnExist: true })
  await verifyPinnedLocalFile(
    pdfCliTarget,
    manifest.buildInputs.pdfRuntimeCli.sha256,
    'staged managed PDF CLI'
  )
  const legal = await downloadPinnedFile(
    manifest.node.licenseFile,
    archiveCachePath(downloadDirectory, manifest.node.licenseFile)
  )
  const legalTarget = join(staging, ...manifest.node.licenseFile.target.split('/'))
  await mkdir(dirname(legalTarget), { recursive: true })
  await cp(legal, legalTarget, { force: false, errorOnExist: true })
}
