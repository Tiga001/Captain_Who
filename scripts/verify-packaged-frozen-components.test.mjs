/* eslint-disable @typescript-eslint/explicit-function-return-type -- Test fixtures intentionally use compact callbacks. */

import assert from 'node:assert/strict'
import { join } from 'node:path'
import test from 'node:test'

import {
  packagedFrozenComponentDirectories,
  packagedFrozenComponentTargetArch,
  packagedFrozenComponentTargetPlatform,
  verifyPackagedFrozenComponentsAfterPack,
  verifyPackagedFrozenComponentsAfterSign
} from './verify-packaged-frozen-components.mjs'

function context(overrides = {}) {
  return {
    electronPlatformName: 'darwin',
    arch: 3,
    appOutDir: '/build/mac-arm64',
    packager: {
      appInfo: { productFilename: 'MyCopilot' },
      platformSpecificBuildOptions: {}
    },
    ...overrides
  }
}

test('packaged frozen component directories stay inside the exact application resources boundary', () => {
  const directories = packagedFrozenComponentDirectories(context())
  const resources = join('/build/mac-arm64', 'MyCopilot.app', 'Contents', 'Resources', 'components')
  assert.deepEqual(directories, {
    artifactRuntime: join(resources, 'artifact-runtime'),
    officeCli: join(resources, 'officecli')
  })
  assert.throws(
    () =>
      packagedFrozenComponentDirectories(
        context({ packager: { appInfo: { productFilename: '../escaped' } } })
      ),
    /productFilename/
  )
})

test('packaged frozen component target rejects cross-platform and cross-architecture builds', () => {
  assert.equal(packagedFrozenComponentTargetPlatform(context(), 'darwin'), 'darwin')
  assert.equal(packagedFrozenComponentTargetArch(context(), 'arm64'), 'arm64')
  assert.throws(() => packagedFrozenComponentTargetPlatform(context(), 'linux'), /Cross-platform/)
  assert.throws(() => packagedFrozenComponentTargetArch(context(), 'x64'), /Cross-architecture/)
})

test('afterPack verifies unsigned receipts and afterSign requires the signed OfficeCLI receipt', async () => {
  const calls = []
  const dependencies = {
    hostPlatform: 'darwin',
    hostArch: 'arm64',
    async artifactVerifier(options) {
      calls.push({ component: 'artifact-runtime', options })
      return { receipt: { bundleRevision: 'artifact' } }
    },
    async officeCliVerifier(options) {
      calls.push({ component: 'officecli', options })
      return { receipt: { bundleRevision: 'officecli' } }
    }
  }

  await verifyPackagedFrozenComponentsAfterPack(context(), dependencies)
  await verifyPackagedFrozenComponentsAfterSign(context(), dependencies)
  assert.equal(calls.length, 4)
  assert.equal(calls[0].options.verifyOnly, true)
  assert.equal(calls[1].options.signed, false)
  assert.equal(calls[3].options.signed, true)
  assert.equal(
    calls.every(({ options }) => options.platform === 'darwin' && options.arch === 'arm64'),
    true
  )
})

test('afterSign preserves unsigned OfficeCLI semantics when signing is explicitly disabled', async () => {
  let signed
  await verifyPackagedFrozenComponentsAfterSign(
    context({
      packager: {
        appInfo: { productFilename: 'MyCopilot' },
        platformSpecificBuildOptions: { identity: null }
      }
    }),
    {
      hostPlatform: 'darwin',
      hostArch: 'arm64',
      async artifactVerifier() {
        return { receipt: {} }
      },
      async officeCliVerifier(options) {
        signed = options.signed
        return { receipt: {} }
      }
    }
  )
  assert.equal(signed, false)
})
