/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node's test runner infers fixture helper contracts. */

import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'

import {
  isMacCodeSigningExplicitlyDisabled,
  packagedApplicationAsarPath,
  packagedMacIconPath,
  verifyPackagedAutoUpdateMetadataDisabled,
  verifyPackagedMacIcon,
  verifyPackagedManagedPlaywrightMcp
} from './verify-packaged-app.mjs'

const requireFromBuilder = createRequire(import.meta.resolve('electron-builder/package.json'))
const { createPackage } = requireFromBuilder('@electron/asar')
const { load: parseYaml } = requireFromBuilder('js-yaml')

function createPackContext(appOutDir) {
  return {
    electronPlatformName: 'darwin',
    appOutDir,
    packager: {
      config: {
        publish: null,
        mac: { target: ['dmg'] },
        dmg: { writeUpdateInfo: false },
        nsis: { differentialPackage: false }
      },
      appInfo: {
        productFilename: 'Captain Who'
      }
    }
  }
}

test('electron-builder freezes the Captain Who release identity and disables update metadata', async () => {
  const configuration = parseYaml(
    await readFile(new URL('../electron-builder.yml', import.meta.url), 'utf8')
  )

  assert.equal(configuration.appId, 'io.github.tiga001.captainwho')
  assert.equal(configuration.productName, 'Captain Who')
  assert.equal(configuration.copyright, 'Copyright © 2026 ShenhuaJiao')
  assert.equal(configuration.publish, null)
  assert.equal(configuration.win.executableName, 'CaptainWho')
  assert.equal(configuration.win.artifactName, 'Captain-Who-${version}-${arch}.${ext}')
  assert.equal(configuration.nsis.artifactName, 'Captain-Who-${version}-${arch}-Setup.${ext}')
  assert.equal(configuration.nsis.differentialPackage, false)
  assert.deepEqual(configuration.mac.target, ['dmg'])
  assert.equal(configuration.dmg.artifactName, 'Captain-Who-${version}-${arch}.${ext}')
  assert.equal(configuration.dmg.writeUpdateInfo, false)
  assert.equal(configuration.linux.artifactName, 'Captain-Who-${version}-${arch}.${ext}')
  assert.equal(configuration.appImage.artifactName, 'Captain-Who-${version}-${arch}.${ext}')
  assert.equal(configuration.linux.maintainer, 'ShenhuaJiao')
  assert.deepEqual(configuration.mac.extendInfo, {
    NSDocumentsFolderUsageDescription: 'Captain Who accesses files you select in Documents.',
    NSDownloadsFolderUsageDescription: 'Captain Who accesses files you select in Downloads.'
  })
})

test('packaged application refuses inferred or embedded auto-update metadata', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'captain-who-packaged-update-metadata-'))
  const context = createPackContext(directory)
  try {
    await verifyPackagedAutoUpdateMetadataDisabled(context)
    await assert.rejects(
      () =>
        verifyPackagedAutoUpdateMetadataDisabled({
          ...context,
          packager: { ...context.packager, config: { publish: [] } }
        }),
      /publish must remain explicitly null/
    )
    await assert.rejects(
      () =>
        verifyPackagedAutoUpdateMetadataDisabled({
          ...context,
          packager: {
            ...context.packager,
            config: { ...context.packager.config, mac: { publish: [] } }
          }
        }),
      /mac\.publish must remain unset/
    )
    await assert.rejects(
      () =>
        verifyPackagedAutoUpdateMetadataDisabled({
          ...context,
          packager: {
            ...context.packager,
            config: {
              ...context.packager.config,
              dmg: { writeUpdateInfo: true }
            }
          }
        }),
      /dmg\.writeUpdateInfo must remain false/
    )
    await assert.rejects(
      () =>
        verifyPackagedAutoUpdateMetadataDisabled({
          ...context,
          packager: {
            ...context.packager,
            config: {
              ...context.packager.config,
              mac: { target: ['dmg', 'zip'] }
            }
          }
        }),
      /mac\.target must remain DMG-only/
    )
    await assert.rejects(
      () =>
        verifyPackagedAutoUpdateMetadataDisabled({
          ...context,
          packager: {
            ...context.packager,
            config: {
              ...context.packager.config,
              nsis: { differentialPackage: true }
            }
          }
        }),
      /nsis\.differentialPackage must remain false/
    )

    const updateConfiguration = join(
      directory,
      'Captain Who.app',
      'Contents',
      'Resources',
      'app-update.yml'
    )
    await mkdir(dirname(updateConfiguration), { recursive: true })
    await writeFile(updateConfiguration, 'provider: github\n')
    await assert.rejects(
      () => verifyPackagedAutoUpdateMetadataDisabled(context),
      /unexpectedly contains app-update\.yml/
    )
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})

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

test('packaged managed Playwright MCP verification accepts only the frozen production graph', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-packaged-playwright-mcp-'))
  const context = createPackContext(directory)
  const source = join(directory, 'source')
  const packages = [
    ['@playwright/mcp', '@playwright/mcp', '0.0.79'],
    ['@modelcontextprotocol/sdk', '@modelcontextprotocol/sdk', '1.29.0'],
    ['playwright', 'playwright', '1.63.0-alpha-2026-08-05'],
    ['playwright-core', 'playwright-core', '1.63.0-alpha-2026-08-05']
  ]
  try {
    for (const [pathName, packageName, version] of packages) {
      const packageDirectory = join(source, 'node_modules', pathName)
      await mkdir(packageDirectory, { recursive: true })
      await writeFile(
        join(packageDirectory, 'package.json'),
        JSON.stringify({
          name: packageName,
          version,
          ...(packageName === '@playwright/mcp'
            ? {
                dependencies: {
                  playwright: '1.63.0-alpha-2026-08-05',
                  'playwright-core': '1.63.0-alpha-2026-08-05'
                }
              }
            : {})
        })
      )
    }
    for (const entry of [
      'node_modules/@playwright/mcp/index.js',
      'node_modules/@modelcontextprotocol/sdk/dist/esm/client/index.js',
      'node_modules/playwright/lib/index.js',
      'node_modules/playwright-core/index.js'
    ]) {
      await mkdir(dirname(join(source, entry)), { recursive: true })
      await writeFile(join(source, entry), 'export {}')
    }
    await mkdir(dirname(packagedApplicationAsarPath(context)), { recursive: true })
    await createPackage(source, packagedApplicationAsarPath(context))

    await verifyPackagedManagedPlaywrightMcp(context)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})

test('packaged managed Playwright MCP verification rejects a missing fixed entry', async () => {
  const context = createPackContext('/tmp/mycopilot-invalid-packaged-runtime')
  const asarApi = {
    listPackage: () => ['/node_modules/@playwright/mcp/package.json'],
    extractFile: () => Buffer.from('{"name":"@playwright/mcp","version":"0.0.79"}')
  }
  await assert.rejects(
    () => verifyPackagedManagedPlaywrightMcp(context, asarApi),
    /missing node_modules\/@modelcontextprotocol\/sdk\/package\.json/
  )
})
