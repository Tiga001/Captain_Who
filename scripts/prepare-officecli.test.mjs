/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node's JavaScript test runner infers these test helper types. */

import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { chmod, mkdtemp, mkdir, readFile, readdir, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import {
  OFFICECLI_MAX_DOWNLOAD_BYTES,
  prepareOfficeCli,
  prepareOfficeCliMacSigning,
  refreshOfficeCliReceiptAfterSigning,
  loadAndValidateManifest,
  selectAsset,
  validateDownloadUrl,
  validateManifest,
  verifyPackagedOfficeCli
} from './prepare-officecli.mjs'

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)))
const manifestPath = join(repositoryRoot, 'resources', 'officecli-manifest.json')

async function readRawManifest() {
  return JSON.parse(await readFile(manifestPath, 'utf8'))
}

function descriptor(bytes) {
  return {
    size: bytes.length,
    sha256: createHash('sha256').update(bytes).digest('hex')
  }
}

async function officeCliSigningFixture() {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-officecli-signing-'))
  const outputDirectory = join(directory, 'current')
  await mkdir(outputDirectory)
  const manifest = await readRawManifest()
  const executable = Buffer.concat([Buffer.from('cffaedfe', 'hex'), Buffer.from('fixture')])
  const files = new Map([
    ['officecli', executable],
    ['LICENSE', Buffer.from('fixture license\n')],
    ['NOTICE', Buffer.from('fixture notice\n')],
    ['THIRD-PARTY-NOTICES.txt', Buffer.from('fixture third-party notices\n')]
  ])
  Object.assign(manifest.assets['darwin-arm64'], descriptor(executable))
  for (const legal of manifest.legalFiles) {
    Object.assign(legal, descriptor(files.get(legal.targetName)))
  }
  const fixtureManifestPath = join(directory, 'officecli-manifest.json')
  await writeFile(fixtureManifestPath, `${JSON.stringify(manifest, null, 2)}\n`)
  for (const [name, bytes] of files) {
    await writeFile(join(outputDirectory, name), bytes)
  }
  await chmod(join(outputDirectory, 'officecli'), 0o755)
  await prepareOfficeCli({
    manifestPath: fixtureManifestPath,
    outputDirectory,
    platform: 'darwin',
    arch: 'arm64'
  })
  return { directory, fixtureManifestPath, outputDirectory }
}

test('pinned manifest contains exactly one asset for every supported target', async () => {
  const manifest = await loadAndValidateManifest(manifestPath)
  assert.equal(manifest.component.version, '1.0.139')
  assert.equal(manifest.component.maxDownloadBytes, OFFICECLI_MAX_DOWNLOAD_BYTES)

  const expectedTargets = [
    [
      'darwin',
      'arm64',
      'officecli-mac-arm64',
      33641808,
      '393874f79db58222bdbede7f4f942f2536580386923857d1b5ad9754efe80c19'
    ],
    [
      'darwin',
      'x64',
      'officecli-mac-x64',
      34580608,
      '6a931d424975dded6ae413c8c1f63d00dfb30a4bd4bd50352964782d13299f5c'
    ],
    [
      'linux',
      'arm64',
      'officecli-linux-arm64',
      34610695,
      '39008c7f76d202858637810553ef14e2cd3e7f61485fdcf2011f26967a7babd1'
    ],
    [
      'linux',
      'x64',
      'officecli-linux-x64',
      35194021,
      'da07d4f787d7c85724104294ac023c89971ddfaee93ebb183b289282b8f869cc'
    ],
    [
      'win32',
      'arm64',
      'officecli-win-arm64.exe',
      33701812,
      '6d80a93ba0c9cafb2b52048efbb403cd761b35126130fa8166383599aa91d96e'
    ],
    [
      'win32',
      'x64',
      'officecli-win-x64.exe',
      33259432,
      '864e0580c8e8c91a6aa4a4c1e8900551c8d4aa648ff10136ceed3a6ba5310888'
    ]
  ]

  for (const [platform, arch, sourceName, size, sha256] of expectedTargets) {
    const asset = selectAsset(manifest, platform, arch)
    assert.equal(asset.sourceName, sourceName)
    assert.equal(asset.size, size)
    assert.equal(asset.sha256, sha256)
    assert.equal(
      asset.url,
      `https://github.com/iOfficeAI/OfficeCLI/releases/download/v1.0.139/${sourceName}`
    )
    assert.equal(asset.targetName, platform === 'win32' ? 'officecli.exe' : 'officecli')
  }
})

test('legal files are pinned to the audited upstream digests', async () => {
  const manifest = await loadAndValidateManifest(manifestPath)
  assert.deepEqual(
    manifest.legalFiles.map(({ targetName, size, sha256 }) => ({ targetName, size, sha256 })),
    [
      {
        targetName: 'LICENSE',
        size: 11375,
        sha256: '7e282402a5a6db33995fe638bb3fe79013f9884d8f7d15a42e481c1e86aadda1'
      },
      {
        targetName: 'NOTICE',
        size: 455,
        sha256: '3a4715b268e148a8e9566f5e835f766f5c95c3da4d6e5ddd908806a258a2f07b'
      },
      {
        targetName: 'THIRD-PARTY-NOTICES.txt',
        size: 2179,
        sha256: '7e79e54cddf05f25681198429eb42e1401c1c53a3b29bf63744dc09411c029d8'
      }
    ]
  )
  for (const legalFile of manifest.legalFiles) {
    assert.equal(
      legalFile.url,
      `https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/v1.0.139/${legalFile.sourceName}`
    )
  }
})

test('selection rejects unsupported runtime targets', async () => {
  const manifest = await loadAndValidateManifest(manifestPath)
  assert.throws(() => selectAsset(manifest, 'freebsd', 'x64'), /not packaged/)
  assert.throws(() => selectAsset(manifest, 'linux', 'riscv64'), /not packaged/)
})

test('download policy rejects insecure, credentialed, ported, and foreign URLs', () => {
  assert.throws(() => validateDownloadUrl('http://github.com/file'), /must use HTTPS/)
  assert.throws(() => validateDownloadUrl('https://user:secret@github.com/file'), /credentials/)
  assert.throws(() => validateDownloadUrl('https://github.com:8443/file'), /custom port/)
  assert.throws(() => validateDownloadUrl('https://example.com/file'), /not allowlisted/)
  assert.equal(
    validateDownloadUrl('https://release-assets.githubusercontent.com/file').hostname,
    'release-assets.githubusercontent.com'
  )
})

test('manifest rejects oversized or unpinned assets', async () => {
  const oversized = await readRawManifest()
  oversized.assets['darwin-arm64'].size = OFFICECLI_MAX_DOWNLOAD_BYTES + 1
  assert.throws(() => validateManifest(oversized), /exceeds the 64 MiB/)

  const unpinned = await readRawManifest()
  unpinned.component.version = 'latest'
  assert.throws(() => validateManifest(unpinned), /pinned to version 1\.0\.139/)
})

test('packaged OfficeCLI signing freezes one Mach-O and atomically refreshes provenance', async () => {
  const fixture = await officeCliSigningFixture()
  const prepared = await prepareOfficeCliMacSigning({
    manifestPath: fixture.fixtureManifestPath,
    outputDirectory: fixture.outputDirectory,
    platform: 'darwin',
    arch: 'arm64'
  })
  assert.deepEqual(
    prepared.targets.map(({ relativePath }) => relativePath),
    ['officecli']
  )
  assert.equal(prepared.receipt.schemaVersion, 2)
  assert.deepEqual(prepared.receipt.signedMachO, [])

  const executablePath = join(fixture.outputDirectory, 'officecli')
  const unsignedBytes = await readFile(executablePath)
  await writeFile(executablePath, Buffer.concat([unsignedBytes, Buffer.from('signed fixture')]))
  await chmod(executablePath, 0o755)
  const refreshed = await refreshOfficeCliReceiptAfterSigning({
    manifestPath: fixture.fixtureManifestPath,
    outputDirectory: fixture.outputDirectory,
    originalReceipt: prepared.receipt,
    signedPaths: prepared.targets.map(({ relativePath }) => relativePath),
    platform: 'darwin',
    arch: 'arm64'
  })

  assert.deepEqual(refreshed.signedMachO, ['officecli'])
  assert.deepEqual(refreshed.sourceFiles, prepared.receipt.sourceFiles)
  assert.notDeepEqual(refreshed.files[0], prepared.receipt.files[0])
  assert.deepEqual(refreshed.files.slice(1), prepared.receipt.files.slice(1))
  assert.notEqual(refreshed.bundleRevision, prepared.receipt.bundleRevision)
  assert.equal(
    (
      await verifyPackagedOfficeCli({
        manifestPath: fixture.fixtureManifestPath,
        outputDirectory: fixture.outputDirectory,
        platform: 'darwin',
        arch: 'arm64',
        signed: true
      })
    ).receipt.bundleRevision,
    refreshed.bundleRevision
  )
  await assert.rejects(
    prepareOfficeCli({
      manifestPath: fixture.fixtureManifestPath,
      outputDirectory: fixture.outputDirectory,
      platform: 'darwin',
      arch: 'arm64',
      verifyOnly: true
    }),
    /Unsigned OfficeCLI receipt/
  )
  assert.equal(
    (await readdir(fixture.outputDirectory)).some((name) => name.endsWith('.tmp')),
    false
  )
})

test('OfficeCLI signing rejects missing Mach-O and changes outside the exact allowlist', async () => {
  const missing = await officeCliSigningFixture()
  const missingPrepared = await prepareOfficeCliMacSigning({
    manifestPath: missing.fixtureManifestPath,
    outputDirectory: missing.outputDirectory,
    platform: 'darwin',
    arch: 'arm64'
  })
  await assert.rejects(
    refreshOfficeCliReceiptAfterSigning({
      manifestPath: missing.fixtureManifestPath,
      outputDirectory: missing.outputDirectory,
      originalReceipt: missingPrepared.receipt,
      signedPaths: [],
      platform: 'darwin',
      arch: 'arm64'
    }),
    /at least one mutated Mach-O path/
  )

  const polluted = await officeCliSigningFixture()
  const prepared = await prepareOfficeCliMacSigning({
    manifestPath: polluted.fixtureManifestPath,
    outputDirectory: polluted.outputDirectory,
    platform: 'darwin',
    arch: 'arm64'
  })
  const receiptPath = join(polluted.outputDirectory, 'component-receipt.json')
  const frozenReceipt = await readFile(receiptPath)
  const executablePath = join(polluted.outputDirectory, 'officecli')
  await writeFile(
    executablePath,
    Buffer.concat([await readFile(executablePath), Buffer.from('signed fixture')])
  )
  await chmod(executablePath, 0o755)
  await writeFile(join(polluted.outputDirectory, 'NOTICE'), 'unexpected mutation\n')
  await assert.rejects(
    refreshOfficeCliReceiptAfterSigning({
      manifestPath: polluted.fixtureManifestPath,
      outputDirectory: polluted.outputDirectory,
      originalReceipt: prepared.receipt,
      signedPaths: ['officecli'],
      platform: 'darwin',
      arch: 'arm64'
    }),
    /outside the Mach-O signing allowlist changed/
  )
  assert.deepEqual(await readFile(receiptPath), frozenReceipt)
})

test('OfficeCLI signing rejects an extra Mach-O even when the frozen file names remain present', async () => {
  const fixture = await officeCliSigningFixture()
  const prepared = await prepareOfficeCliMacSigning({
    manifestPath: fixture.fixtureManifestPath,
    outputDirectory: fixture.outputDirectory,
    platform: 'darwin',
    arch: 'arm64'
  })
  const executablePath = join(fixture.outputDirectory, 'officecli')
  await writeFile(
    executablePath,
    Buffer.concat([await readFile(executablePath), Buffer.from('signed fixture')])
  )
  await chmod(executablePath, 0o755)
  await writeFile(
    join(fixture.outputDirectory, 'NOTICE'),
    Buffer.concat([Buffer.from('cffaedfe', 'hex'), Buffer.from('unexpected Mach-O')])
  )
  await assert.rejects(
    refreshOfficeCliReceiptAfterSigning({
      manifestPath: fixture.fixtureManifestPath,
      outputDirectory: fixture.outputDirectory,
      originalReceipt: prepared.receipt,
      signedPaths: ['officecli'],
      platform: 'darwin',
      arch: 'arm64'
    }),
    /Mach-O file set changed/
  )
})
