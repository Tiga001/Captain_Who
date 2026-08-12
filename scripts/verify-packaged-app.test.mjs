/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node's test runner infers fixture helper contracts. */

import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'

import {
  isMacCodeSigningExplicitlyDisabled,
  packagedMacIconPath,
  verifyPackagedMacIcon
} from './verify-packaged-app.mjs'

function createPackContext(appOutDir) {
  return {
    electronPlatformName: 'darwin',
    appOutDir,
    packager: {
      appInfo: {
        productFilename: 'MyCopilot'
      }
    }
  }
}

test('packaged macOS icon verification accepts the configured brand icon', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-packaged-icon-'))
  const context = createPackContext(directory)
  const sourceIconPath = join(directory, 'source.icns')
  const outputIconPath = packagedMacIconPath(context)
  await mkdir(dirname(outputIconPath), { recursive: true })
  await writeFile(sourceIconPath, 'brand icon')
  await writeFile(outputIconPath, 'brand icon')

  await verifyPackagedMacIcon(context, sourceIconPath)
})

test('packaged macOS icon verification rejects stale Electron artwork', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-stale-packaged-icon-'))
  const context = createPackContext(directory)
  const sourceIconPath = join(directory, 'source.icns')
  const outputIconPath = packagedMacIconPath(context)
  await mkdir(dirname(outputIconPath), { recursive: true })
  await writeFile(sourceIconPath, 'brand icon')
  await writeFile(outputIconPath, 'stock Electron icon')

  await assert.rejects(
    () => verifyPackagedMacIcon(context, sourceIconPath),
    /does not match build\/icon\.icns/
  )
})

test('macOS signature verification is skipped only for an explicitly null identity', () => {
  assert.equal(
    isMacCodeSigningExplicitlyDisabled({
      packager: { platformSpecificBuildOptions: { identity: null } }
    }),
    true
  )
  assert.equal(
    isMacCodeSigningExplicitlyDisabled({
      packager: { platformSpecificBuildOptions: {} }
    }),
    false
  )
  assert.equal(
    isMacCodeSigningExplicitlyDisabled({
      packager: { platformSpecificBuildOptions: { identity: 'Developer ID Application' } }
    }),
    false
  )
  assert.equal(isMacCodeSigningExplicitlyDisabled({}), false)
})
