/* eslint-disable @typescript-eslint/explicit-function-return-type -- Electron loads this JavaScript command boundary directly; runtime validation defines its contract. */

import { mkdirSync, readFileSync, realpathSync } from 'node:fs'
import { isAbsolute, resolve } from 'node:path'

export function readDevelopmentElectronAppName(
  packageMetadataPath = resolve(import.meta.dirname, '..', 'package.json')
) {
  const metadata = JSON.parse(readFileSync(packageMetadataPath, 'utf8'))
  if (typeof metadata.name !== 'string' || metadata.name.trim().length === 0) {
    throw new Error('The workspace package must declare a non-empty application name')
  }
  return metadata.name
}

export function resolveElectronAppDataRoot(
  electronApp,
  applicationName,
  canonicalize = realpathSync,
  createDirectory = mkdirSync
) {
  if (typeof applicationName !== 'string' || applicationName.trim().length === 0) {
    throw new Error('Development Electron application name must be non-empty')
  }

  // A standalone `electron script.mjs` process otherwise identifies itself as
  // "Electron" and resolves a different userData directory than the development
  // app. Set the package identity before the first userData lookup.
  electronApp.setName(applicationName)
  const configuredRoot = electronApp.getPath('userData')
  if (!isAbsolute(configuredRoot)) {
    throw new Error('Electron application data root must be absolute')
  }
  createDirectory(configuredRoot, { recursive: true, mode: 0o700 })
  return canonicalize(configuredRoot)
}

export function buildDevelopmentStorageResetCommand(
  electronAppDataRoot,
  forwardedArguments,
  workspaceRoot = resolve(import.meta.dirname, '..')
) {
  if (!isAbsolute(electronAppDataRoot)) {
    throw new Error('Electron application data root must be absolute')
  }
  if (forwardedArguments.some((argument) => typeof argument !== 'string')) {
    throw new Error('Storage reset arguments must be strings')
  }
  const resetArguments = forwardedArguments.filter((argument) => argument !== '--')
  if (
    resetArguments.some((argument) => argument !== '--confirm-reset') ||
    resetArguments.filter((argument) => argument === '--confirm-reset').length > 1
  ) {
    throw new Error('Only one optional --confirm-reset argument is supported')
  }
  return {
    executable: 'cargo',
    arguments: [
      'run',
      '-p',
      'mycopilot-core-server',
      '--bin',
      'storage-reset-dev',
      '--quiet',
      '--',
      '--app-data-root',
      electronAppDataRoot,
      ...resetArguments
    ],
    cwd: workspaceRoot
  }
}
