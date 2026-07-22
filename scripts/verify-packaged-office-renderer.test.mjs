import assert from 'node:assert/strict'
import { join } from 'node:path'
import test from 'node:test'

import {
  packagedOfficeRendererDirectory,
  packagedOfficeRendererTargetArch,
  packagedOfficeRendererTargetPlatform
} from './verify-packaged-office-renderer.mjs'

test('packaged renderer resolves inside the platform resources directory', () => {
  assert.equal(
    packagedOfficeRendererDirectory({
      appOutDir: '/build/mac-arm64',
      electronPlatformName: 'darwin',
      packager: { appInfo: { productFilename: 'MyCopilot' } }
    }),
    join(
      '/build/mac-arm64',
      'MyCopilot.app',
      'Contents',
      'Resources',
      'components',
      'office-renderer'
    )
  )
  assert.equal(
    packagedOfficeRendererDirectory({
      appOutDir: '/build/win-unpacked',
      electronPlatformName: 'win32'
    }),
    join('/build/win-unpacked', 'resources', 'components', 'office-renderer')
  )
})

test('packaged renderer path resolution fails closed on malformed hook context', () => {
  assert.throws(() => packagedOfficeRendererDirectory(undefined), /pack context is required/)
  assert.throws(
    () => packagedOfficeRendererDirectory({ electronPlatformName: 'linux' }),
    /appOutDir is required/
  )
  assert.throws(
    () =>
      packagedOfficeRendererDirectory({
        appOutDir: '/build/mac-arm64',
        electronPlatformName: 'darwin',
        packager: { appInfo: { productFilename: '../MyCopilot' } }
      }),
    /productFilename is required/
  )
})

test('packaged renderer maps supported electron-builder architectures', () => {
  assert.equal(packagedOfficeRendererTargetArch({ arch: 1 }, 'x64'), 'x64')
  assert.equal(packagedOfficeRendererTargetArch({ arch: 3 }, 'arm64'), 'arm64')
})

test('packaged renderer rejects cross-architecture and unknown targets', () => {
  assert.throws(
    () => packagedOfficeRendererTargetArch({ arch: 1 }, 'arm64'),
    /Cross-architecture Office renderer packaging is not supported/
  )
  assert.throws(
    () => packagedOfficeRendererTargetArch({ arch: 3 }, 'x64'),
    /Cross-architecture Office renderer packaging is not supported/
  )
  for (const arch of [undefined, 0, 2, 4, 'x64']) {
    assert.throws(
      () => packagedOfficeRendererTargetArch({ arch }, 'x64'),
      /Unsupported electron-builder target architecture/
    )
  }
  assert.throws(
    () => packagedOfficeRendererTargetArch({ arch: 1 }, 'riscv64'),
    /Unsupported Office renderer host architecture/
  )
})

test('packaged renderer maps the target platform and rejects cross-platform packages', () => {
  assert.equal(
    packagedOfficeRendererTargetPlatform({ electronPlatformName: 'darwin' }, 'darwin'),
    'darwin'
  )
  assert.equal(
    packagedOfficeRendererTargetPlatform({ electronPlatformName: 'mas' }, 'darwin'),
    'darwin'
  )
  assert.equal(
    packagedOfficeRendererTargetPlatform({ electronPlatformName: 'linux' }, 'linux'),
    'linux'
  )
  assert.equal(
    packagedOfficeRendererTargetPlatform({ electronPlatformName: 'win32' }, 'win32'),
    'win32'
  )
  assert.throws(
    () => packagedOfficeRendererTargetPlatform({ electronPlatformName: 'win32' }, 'darwin'),
    /Cross-platform Office renderer packaging is not supported/
  )
  assert.throws(
    () => packagedOfficeRendererTargetPlatform({ electronPlatformName: 'freebsd' }, 'darwin'),
    /Unsupported electron-builder target platform/
  )
  assert.throws(
    () => packagedOfficeRendererTargetPlatform({ electronPlatformName: 'darwin' }, 'freebsd'),
    /Unsupported Office renderer host platform/
  )
})
