import { isBuiltin } from 'node:module'
/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node loader hook signatures are defined by the managed runtime protocol. */

import { realpathSync } from 'node:fs'
import { dirname, isAbsolute, join, relative, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const PRESENTATION_SDK_SPECIFIER = '@mycopilot/presentation-sdk'
const presentationSdkUrl = new URL('./presentation-sdk.mjs', import.meta.url)
const PRESENTATION_EDITOR_MODE = process.env.MYCOPILOT_PRESENTATION_EDIT_MODE === 'v1'
const presentationEditorEntry = process.env.MYCOPILOT_PRESENTATION_EDITOR_ENTRY
const presentationEditorEntryUrl =
  PRESENTATION_EDITOR_MODE && presentationEditorEntry
    ? pathToFileURL(realpathSync(presentationEditorEntry)).href
    : undefined

let managedModuleRoot
let managedAnchorUrl

export function initialize({ moduleRoot }) {
  if (typeof moduleRoot !== 'string' || moduleRoot.length === 0) {
    throw new Error('Managed Artifact Runtime moduleRoot is missing')
  }
  managedModuleRoot = realpathSync(moduleRoot)
  // Resolve workspace-authored bare imports as if they originated beside the pinned
  // node_modules directory. Calling require.resolve from a synchronous registerHooks callback
  // re-enters the same hook on newer Node releases and can recurse indefinitely.
  managedAnchorUrl = pathToFileURL(join(dirname(managedModuleRoot), 'managed-entry.mjs')).href
}

export function resolve(specifier, context, nextResolve) {
  if (specifier === PRESENTATION_SDK_SPECIFIER) {
    if (!PRESENTATION_EDITOR_MODE) {
      throw new Error('Presentation Editor SDK is available only to a Host-verified editor')
    }
    return { shortCircuit: true, url: presentationSdkUrl.href }
  }
  if (PRESENTATION_EDITOR_MODE) {
    if (context.parentURL == null && specifier === presentationEditorEntryUrl) {
      return nextResolve(specifier, context)
    }
    if (context.parentURL === presentationSdkUrl.href && specifier === 'node:fs/promises') {
      return nextResolve(specifier, context)
    }
    throw new Error(`Presentation Editor refused undeclared module import: ${specifier}`)
  }
  if (!isBarePackageSpecifier(specifier) || isBuiltin(specifier)) {
    return nextResolve(specifier, context)
  }
  const parentURL = managedParentUrl(context.parentURL)
  const resolution = nextResolve(specifier, { ...context, parentURL })
  if (!resolution || typeof resolution.url !== 'string' || !resolution.url.startsWith('file:')) {
    throw new Error(`Managed Artifact Runtime could not resolve package as a file: ${specifier}`)
  }
  const resolved = realpathSync(fileURLToPath(resolution.url))
  if (!isWithinManagedModules(resolved)) {
    throw new Error(
      `Managed Artifact Runtime refused package resolution outside moduleRoot: ${specifier}`
    )
  }
  return {
    ...resolution,
    shortCircuit: true,
    url: pathToFileURL(resolved).href
  }
}

function managedParentUrl(parentUrl) {
  if (typeof parentUrl !== 'string' || !parentUrl.startsWith('file:')) {
    return managedAnchorUrl
  }
  const parentPath = fileURLToPath(parentUrl)
  return isWithinManagedModules(parentPath) ? parentUrl : managedAnchorUrl
}

function isWithinManagedModules(path) {
  if (!isAbsolute(path)) return false
  const child = relative(managedModuleRoot, path)
  return child === '' || (!child.startsWith(`..${sep}`) && child !== '..' && !isAbsolute(child))
}

function isBarePackageSpecifier(specifier) {
  return (
    typeof specifier === 'string' &&
    specifier.length > 0 &&
    !specifier.startsWith('.') &&
    !specifier.startsWith('/') &&
    !specifier.includes(':')
  )
}
