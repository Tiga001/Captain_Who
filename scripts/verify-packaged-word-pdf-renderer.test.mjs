import assert from 'node:assert/strict'
import { join } from 'node:path'
import test from 'node:test'

import {
  packagedWordPdfRendererDirectory,
  packagedWordPdfRendererTargetArch
} from './verify-packaged-word-pdf-renderer.mjs'

test('resolves the signed macOS Word PDF component resource boundary', () => {
  assert.equal(
    packagedWordPdfRendererDirectory({
      electronPlatformName: 'darwin',
      appOutDir: '/build/mac-arm64',
      packager: { appInfo: { productFilename: 'MyCopilot' } }
    }),
    join(
      '/build/mac-arm64',
      'MyCopilot.app',
      'Contents',
      'Resources',
      'components',
      'word-pdf-renderer'
    )
  )
  assert.throws(
    () => packagedWordPdfRendererDirectory({ electronPlatformName: 'linux' }),
    /macOS electron-builder pack context/
  )
})

test('accepts only a same-architecture macOS package', () => {
  assert.equal(packagedWordPdfRendererTargetArch({ arch: 1 }, 'x64'), 'x64')
  assert.equal(packagedWordPdfRendererTargetArch({ arch: 3 }, 'arm64'), 'arm64')
  assert.throws(() => packagedWordPdfRendererTargetArch({ arch: 1 }, 'arm64'), /Cross-architecture/)
  assert.throws(
    () => packagedWordPdfRendererTargetArch({ arch: 2 }, 'arm64'),
    /Unsupported electron-builder target architecture/
  )
})
