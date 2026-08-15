/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node runtime hooks use JavaScript contracts. */

import { realpathSync } from 'node:fs'
import Module, { createRequire, isBuiltin, registerHooks } from 'node:module'
import { isAbsolute, join, relative, sep } from 'node:path'
import { initialize as initializeManagedLoader, resolve as resolveManagedLoader } from './node-loader.mjs'

const moduleRoot = process.env.MYCOPILOT_ARTIFACT_NODE_MODULES
if (!moduleRoot) {
  throw new Error('MYCOPILOT_ARTIFACT_NODE_MODULES is required by the managed Artifact Runtime')
}

const managedModuleRoot = realpathSync(moduleRoot)
const managedRequire = createRequire(join(managedModuleRoot, 'package.json'))
const originalResolveFilename = Module._resolveFilename

// A Host-verified Presentation Editor is a plan producer, not a general Node program. Node's
// permission model covers filesystem/process capabilities but intentionally does not claim a
// complete network boundary. Remove the remaining builtin escape hatches before the user-authored
// module is loaded; the registered loader separately denies every import except the fixed SDK.
const presentationEditorMode = process.env.MYCOPILOT_PRESENTATION_EDIT_MODE === 'v1'
if (presentationEditorMode) {
  for (const name of [
    'getBuiltinModule',
    'binding',
    '_linkedBinding',
    'kill',
    '_kill',
    '_debugProcess',
    'execve',
    'dlopen',
    'loadEnvFile',
    'openStdin'
  ]) {
    Object.defineProperty(process, name, {
      value: undefined,
      writable: false,
      enumerable: false,
      configurable: false
    })
  }
  for (const name of ['fetch', 'WebSocket', 'EventSource']) {
    if (name in globalThis) {
      Object.defineProperty(globalThis, name, {
        value: undefined,
        writable: false,
        enumerable: false,
        configurable: false
      })
    }
  }
  for (const name of ['module', 'require']) {
    if (name in globalThis) {
      Object.defineProperty(globalThis, name, {
        value: undefined,
        writable: false,
        enumerable: false,
        configurable: false
      })
    }
  }
}

// The ESM loader below owns bare `import` resolution. Node's ESM hooks do not intercept
// `createRequire()`, so pin the corresponding CommonJS path here as well. The runtime itself is
// versioned and immutable, which lets this deliberately narrow use of Node's internal resolver be
// tested as part of the component contract instead of inheriting a workspace NODE_PATH.
Module._resolveFilename = function resolveManagedCommonJs(request, parent, isMain, options) {
  if (presentationEditorMode) {
    throw new Error(`Presentation Editor refused CommonJS module access: ${request}`)
  }
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

// The pinned Node runtime provides synchronous in-thread hooks. Using them avoids granting the
// worker capability required by the older `module.register()` implementation; Editor code never
// receives a Worker permission merely so the Host can enforce module resolution.
initializeManagedLoader({ moduleRoot: managedModuleRoot })
registerHooks({ resolve: resolveManagedLoader })

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
