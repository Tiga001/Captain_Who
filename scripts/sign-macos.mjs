/* eslint-disable @typescript-eslint/explicit-function-return-type -- electron-builder loads this JavaScript module directly. */

import { fileURLToPath } from 'node:url'
import { dirname, join, resolve } from 'node:path'

export const CORE_SERVER_CODE_SIGN_IDENTIFIER = 'com.mycopilot.next.core-server'

const CORE_SERVER_RELATIVE_PATH = join('Contents', 'Resources', 'core-server')
const CORE_SERVER_ENTITLEMENTS = resolve(
  dirname(fileURLToPath(import.meta.url)),
  '..',
  'build',
  'entitlements.core-server.mac.plist'
)

function requireSigningConfiguration(configuration) {
  if (!configuration || typeof configuration !== 'object') {
    throw new Error('macOS signing configuration is required')
  }
  if (configuration.platform !== 'darwin') {
    throw new Error(`MyCopilot distribution signing requires darwin, got ${configuration.platform}`)
  }
  if (typeof configuration.app !== 'string' || !configuration.app.endsWith('.app')) {
    throw new Error('MyCopilot distribution signing requires a macOS .app path')
  }
  if (
    typeof configuration.identity !== 'string' ||
    configuration.identity.length === 0 ||
    configuration.identity === '-'
  ) {
    throw new Error(
      'MyCopilot distribution signing requires a real Developer ID Application identity; ad-hoc and unsigned identities are forbidden'
    )
  }
}

function hasIdentifierArgument(argumentsList) {
  return argumentsList.some(
    (argument) => argument === '--identifier' || argument.startsWith('--identifier=')
  )
}

export function isCoreServerSigningTarget(appPath, filePath) {
  if (typeof appPath !== 'string' || typeof filePath !== 'string') {
    return false
  }
  return resolve(filePath) === resolve(appPath, CORE_SERVER_RELATIVE_PATH)
}

export function createMacSignOptions(configuration) {
  requireSigningConfiguration(configuration)

  const inheritedOptionsForFile = configuration.optionsForFile
  if (inheritedOptionsForFile !== undefined && typeof inheritedOptionsForFile !== 'function') {
    throw new Error('macOS signing optionsForFile must be a function when provided')
  }

  return {
    ...configuration,
    strictVerify: true,
    optionsForFile(filePath) {
      const inheritedOptions = inheritedOptionsForFile?.(filePath) ?? null
      if (
        inheritedOptions !== null &&
        (typeof inheritedOptions !== 'object' ||
          typeof inheritedOptions.then === 'function' ||
          Array.isArray(inheritedOptions))
      ) {
        throw new Error('macOS signing optionsForFile must return an object or null synchronously')
      }
      if (!isCoreServerSigningTarget(configuration.app, filePath)) {
        return inheritedOptions
      }

      const additionalArguments = [...(inheritedOptions?.additionalArguments ?? [])]
      if (hasIdentifierArgument(additionalArguments)) {
        throw new Error('core-server signing options already contain a code-sign identifier')
      }

      return {
        ...inheritedOptions,
        entitlements: CORE_SERVER_ENTITLEMENTS,
        hardenedRuntime: true,
        additionalArguments: [
          ...additionalArguments,
          '--identifier',
          CORE_SERVER_CODE_SIGN_IDENTIFIER
        ]
      }
    }
  }
}

export function resolvePromiseSigningFunction(osxSignModule) {
  const defaultExport = osxSignModule?.default
  const signApplication =
    osxSignModule?.signAsync ??
    osxSignModule?.signApp ??
    defaultExport?.signAsync ??
    defaultExport?.signApp
  if (typeof signApplication !== 'function') {
    throw new Error('@electron/osx-sign did not expose its Promise-based signing function')
  }
  return signApplication
}

export async function sign(configuration) {
  const options = createMacSignOptions(configuration)
  const osxSignModule = await import('@electron/osx-sign')
  // Never use the legacy `sign` export: it is callback-based, returns `undefined`, and would let
  // electron-builder continue to afterSign while codesign is still running. Only the Promise API
  // gives the packaging transaction a real completion boundary.
  const signApplication = resolvePromiseSigningFunction(osxSignModule)
  await signApplication(options)
}
