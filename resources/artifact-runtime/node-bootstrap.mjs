/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node runtime hooks use JavaScript contracts. */

import { realpathSync } from 'node:fs'
import Module, { createRequire, isBuiltin, register } from 'node:module'
import { isAbsolute, join, relative, sep } from 'node:path'

const moduleRoot = process.env.MYCOPILOT_ARTIFACT_NODE_MODULES
if (!moduleRoot) {
  throw new Error('MYCOPILOT_ARTIFACT_NODE_MODULES is required by the managed Artifact Runtime')
}

const managedModuleRoot = realpathSync(moduleRoot)
const managedRequire = createRequire(join(managedModuleRoot, 'package.json'))
const originalResolveFilename = Module._resolveFilename

// The ESM loader below owns bare `import` resolution. Node's ESM hooks do not intercept
// `createRequire()`, so pin the corresponding CommonJS path here as well. The runtime itself is
// versioned and immutable, which lets this deliberately narrow use of Node's internal resolver be
// tested as part of the component contract instead of inheriting a workspace NODE_PATH.
Module._resolveFilename = function resolveManagedCommonJs(request, parent, isMain, options) {
  if (!isBarePackageSpecifier(request) || isBuiltin(request)) {
    return Reflect.apply(originalResolveFilename, this, [request, parent, isMain, options])
  }

  const parentPath = typeof parent?.filename === 'string' ? parent.filename : undefined
  const resolved = isWithinManagedModules(parentPath)
    ? Reflect.apply(originalResolveFilename, this, [request, parent, isMain, options])
    : managedRequire.resolve(request)
  if (typeof resolved !== 'string' || !isWithinManagedModules(resolved)) {
    throw new Error(
      `Managed Artifact Runtime refused CommonJS package resolution outside moduleRoot: ${request}`
    )
  }
  return resolved
}

register(new URL('./node-loader.mjs', import.meta.url), {
  data: { moduleRoot: managedModuleRoot }
})

function isWithinManagedModules(path) {
  if (typeof path !== 'string' || !isAbsolute(path)) return false
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
