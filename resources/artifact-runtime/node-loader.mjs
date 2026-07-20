import { createRequire, isBuiltin } from 'node:module'
/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node loader hook signatures are defined by the managed runtime protocol. */

import { realpathSync } from 'node:fs'
import { isAbsolute, join, relative, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

let managedRequire
let managedModuleRoot

export function initialize({ moduleRoot }) {
  if (typeof moduleRoot !== 'string' || moduleRoot.length === 0) {
    throw new Error('Managed Artifact Runtime moduleRoot is missing')
  }
  managedModuleRoot = realpathSync(moduleRoot)
  managedRequire = createRequire(join(managedModuleRoot, 'package.json'))
}

export async function resolve(specifier, context, nextResolve) {
  if (!isBarePackageSpecifier(specifier) || isBuiltin(specifier)) {
    return nextResolve(specifier, context)
  }
  const resolver = managedResolverForParent(context.parentURL)
  const resolved = resolver.resolve(specifier)
  if (!isWithinManagedModules(resolved)) {
    throw new Error(
      `Managed Artifact Runtime refused package resolution outside moduleRoot: ${specifier}`
    )
  }
  return {
    shortCircuit: true,
    url: pathToFileURL(resolved).href
  }
}

function managedResolverForParent(parentUrl) {
  if (typeof parentUrl !== 'string' || !parentUrl.startsWith('file:')) {
    return managedRequire
  }
  const parentPath = fileURLToPath(parentUrl)
  return isWithinManagedModules(parentPath) ? createRequire(parentUrl) : managedRequire
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
