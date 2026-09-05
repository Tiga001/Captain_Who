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

  const resetArguments =
    forwardedArguments[0] === '--' ? forwardedArguments.slice(1) : [...forwardedArguments]
  let confirmReset = false
  let configurationSource
  for (let index = 0; index < resetArguments.length; index += 1) {
    const argument = resetArguments[index]
    if (argument === '--confirm-reset') {
      if (confirmReset) {
        throw new Error('--confirm-reset may be supplied only once')
      }
      confirmReset = true
      continue
    }
    if (argument === '--configuration-source') {
      if (configurationSource !== undefined) {
        throw new Error('--configuration-source may be supplied only once')
      }
      const source = resetArguments[index + 1]
      if (source === undefined || !isAbsolute(source)) {
        throw new Error('--configuration-source requires an absolute backup path')
      }
      configurationSource = source
      index += 1
      continue
    }
    throw new Error(`Unsupported storage reset argument: ${argument}`)
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
