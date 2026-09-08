/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node's test runner infers helper contracts. */

import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { chmod, mkdtemp, mkdir, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { Readable } from 'node:stream'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import {
  computeBundleRevision,
  downloadPinnedArchive,
  loadOfficeRendererManifest,
  prepareOfficeRenderer,
  refreshOfficeRendererReceiptAfterSigning,
  resolveOfficePlaywrightCoreRoot,
  selectOfficeRendererTarget,
  syncRegularFile,
  validateOfficeRendererManifest
} from './prepare-office-renderer.mjs'

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)))
const manifestPath = join(repositoryRoot, 'resources', 'office-renderer-manifest.json')

async function rawManifest() {
  return JSON.parse(await readFile(manifestPath, 'utf8'))
}

test('renderer file synchronization preserves bytes on the native platform', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-sync-'))
  const path = join(directory, 'payload.bin')
  const bytes = Buffer.from('verified renderer payload')
  await writeFile(path, bytes)
  await syncRegularFile(path)
  assert.deepEqual(await readFile(path), bytes)
})

test('Office Playwright resolves its own core in nested and sibling package layouts', async () => {
  for (const nested of [true, false]) {
    const directory = await mkdtemp(join(tmpdir(), 'mycopilot-office-playwright-layout-'))
    const playwright = nested
      ? join(directory, 'node_modules', '@mycopilot', 'office-playwright-runtime')
      : join(directory, 'node_modules', '.pnpm', 'playwright@1.61.1', 'node_modules', 'playwright')
    const core = nested
      ? join(playwright, 'node_modules', 'playwright-core')
      : join(dirname(playwright), 'playwright-core')
    await mkdir(playwright, { recursive: true })
    await mkdir(core, { recursive: true })
    const packageJsonPath = join(playwright, 'package.json')
    await writeFile(packageJsonPath, JSON.stringify({ name: 'playwright', version: '1.61.1' }))
    await writeFile(
      join(core, 'package.json'),
      JSON.stringify({ name: 'playwright-core', version: '1.61.1' })
    )
    // A separate application version must not shadow the Office runtime's dependency.
    const appCore = join(directory, 'node_modules', 'playwright-core')
    await mkdir(appCore, { recursive: true })
    await writeFile(
      join(appCore, 'package.json'),
      JSON.stringify({ name: 'playwright-core', version: '1.63.0' })
    )
    assert.equal(resolveOfficePlaywrightCoreRoot(packageJsonPath), core)
  }
})

async function fixtureInstaller({ installRoot, manifest, target }, options = {}) {
  const installed = join(installRoot, `chromium_headless_shell-${manifest.browser.revision}`)
  const executable = join(installed, ...target.executable.split('/').slice(1))
  await mkdir(dirname(executable), { recursive: true })
  const quotedVersion = manifest.browser.version.replaceAll("'", "'\\''")
  await writeFile(executable, `#!/bin/sh\necho 'Google Chrome for Testing ${quotedVersion}'\n`, {
    mode: 0o755
  })
  await writeFile(join(dirname(executable), 'icudtl.dat'), 'pinned browser fixture\n')
  if (options.emptyDirectory) await mkdir(join(installed, 'unexpected-empty-directory'))
  if (options.symlink) {
    await symlink('icudtl.dat', join(dirname(executable), 'linked-resource'))
  }
  return installed
}

for (const [platform, fileLimit] of [
  ['win32', 320],
  ['darwin', 256],
  ['linux', 256]
]) {
  test(`${platform}: renderer verification enforces its bounded file count and integrity`, async (t) => {
    const manifest = await loadOfficeRendererManifest(manifestPath)
    const target = selectOfficeRendererTarget(manifest, platform, 'x64')
    const outputDirectory = await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-limit-'))
    t.after(() => rm(outputDirectory, { recursive: true, force: true }))
    const content = 'pinned renderer resource\n'
    const descriptor = (path) => ({
      path,
      size: Buffer.byteLength(content),
      sha256: createHash('sha256').update(content).digest('hex')
    })
    const paths = [
      target.executable,
      ...Array.from({ length: fileLimit - 1 }, (_, index) => `browser/resource-${index}.pak`)
    ]
    for (const relative of paths) {
      const path = join(outputDirectory, ...relative.split('/'))
      await mkdir(dirname(path), { recursive: true })
      await writeFile(path, content, { mode: 0o755 })
    }
    const receipt = {
      schemaVersion: 2,
      providerId: manifest.providerId,
      bundleVersion: manifest.bundleVersion,
      platform,
      arch: 'x64',
      browser: { ...manifest.browser, executable: target.executable },
      archive: target.archive,
      files: paths
        .sort((left, right) => Buffer.from(left).compare(Buffer.from(right)))
        .map(descriptor)
    }
    const saveReceipt = async () => {
      await writeFile(
        join(outputDirectory, 'component-receipt.json'),
        JSON.stringify({ ...receipt, bundleRevision: computeBundleRevision(receipt) })
      )
    }
    await saveReceipt()
    const options = { outputDirectory, platform, arch: 'x64', verifyOnly: true }
    const verified = await prepareOfficeRenderer(options)
    assert.equal(verified.receipt.files.length, fileLimit)

    const resource = join(outputDirectory, 'browser', 'resource-0.pak')
    await writeFile(resource, 'changed bytes')
    await assert.rejects(prepareOfficeRenderer(options), /do not match the frozen receipt/)
    await writeFile(resource, content)

    const overflow = 'browser/zz-overflow.pak'
    await writeFile(join(outputDirectory, ...overflow.split('/')), content)
    await assert.rejects(prepareOfficeRenderer(options), /exceeds its file-count limit/)
    receipt.files.push(descriptor(overflow))
    await saveReceipt()
    await assert.rejects(prepareOfficeRenderer(options), /receipt files are invalid/)
  })
}

test('manifest pins one exact Playwright Chromium Headless Shell for every desktop target', async () => {
  const manifest = await loadOfficeRendererManifest(manifestPath)
  assert.equal(manifest.providerId, 'mycopilot.office-render-runtime')
  assert.equal(manifest.bundleVersion, '2026.07.2')
  assert.deepEqual(manifest.browser, {
    family: 'chromium',
    version: '149.0.7827.55',
    playwrightVersion: '1.61.1',
    revision: '1228'
  })
  assert.deepEqual(Object.keys(manifest.targets).sort(), [
    'darwin-arm64',
    'darwin-x64',
    'linux-arm64',
    'linux-x64',
    'win32-arm64',
    'win32-x64'
  ])
  for (const platform of ['darwin', 'linux', 'win32']) {
    for (const arch of ['arm64', 'x64']) {
      const target = selectOfficeRendererTarget(manifest, platform, arch)
      assert.match(target.executable, /^browser\//)
      assert.match(target.archive.url, /^https:\/\/cdn\.playwright\.dev\//)
      assert.ok(target.archive.size > 0)
      assert.match(target.archive.sha256, /^[a-f0-9]{64}$/)
    }
  }
  assert.throws(() => selectOfficeRendererTarget(manifest, 'freebsd', 'x64'), /not packaged/)
})

test('manifest validation fails closed on mutable identities and path escapes', async () => {
  const mutable = await rawManifest()
  mutable.browser.playwrightVersion = 'latest'
  assert.throws(() => validateOfficeRendererManifest(mutable), /must pin Chromium/)

  const escaped = await rawManifest()
  escaped.targets['darwin-arm64'].executable = '../Google Chrome'
  assert.throws(() => validateOfficeRendererManifest(escaped), /canonical relative path/)

  const extraTarget = await rawManifest()
  extraTarget.targets['freebsd-x64'] = { executable: 'browser/chrome' }
  assert.throws(() => validateOfficeRendererManifest(extraTarget), /must contain exactly/)

  const mutableArchive = await rawManifest()
  mutableArchive.targets['darwin-arm64'].archive.url = 'https://example.com/chromium.zip'
  assert.throws(() => validateOfficeRendererManifest(mutableArchive), /cdn\.playwright\.dev/)

  const weakDigest = await rawManifest()
  weakDigest.targets['darwin-arm64'].archive.sha256 = 'latest'
  assert.throws(() => validateOfficeRendererManifest(weakDigest), /lowercase SHA-256/)
})

test('archive acquisition verifies the frozen byte length and SHA-256 while streaming', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-download-'))
  const payload = Buffer.from('authenticated renderer archive fixture')
  const archive = {
    url: 'https://cdn.playwright.dev/fixed-fixture.zip',
    size: payload.length,
    sha256: createHash('sha256').update(payload).digest('hex')
  }
  const request = (_params, onResponse) => {
    const response = Readable.from([payload])
    response.statusCode = 200
    response.headers = { 'content-length': String(payload.length) }
    onResponse(response)
    return { cancel: () => undefined }
  }
  const output = join(directory, 'archive.zip')
  await downloadPinnedArchive(archive, output, request)
  assert.deepEqual(await readFile(output), payload)

  await assert.rejects(
    downloadPinnedArchive(
      { ...archive, sha256: '0'.repeat(64) },
      join(directory, 'corrupted.zip'),
      request
    ),
    /frozen SHA-256 identity check/
  )
})

test('bundle revision uses canonical JSON and excludes the revision field itself', () => {
  const left = { b: 2, nested: { z: 3, a: 1 }, a: ['x', { y: true }] }
  const right = { a: ['x', { y: true }], nested: { a: 1, z: 3 }, b: 2 }
  const revision = computeBundleRevision(left)
  assert.equal(revision, computeBundleRevision(right))
  assert.equal(revision, computeBundleRevision({ ...left, bundleRevision: 'ignored' }))
  assert.match(revision, /^office-render-runtime-sha256-v1:[a-f0-9]{64}$/)
  assert.notEqual(revision, computeBundleRevision({ ...left, b: 4 }))
  const expected = createHash('sha256')
    .update(JSON.stringify({ a: ['x', { y: true }], b: 2, nested: { a: 1, z: 3 } }))
    .digest('hex')
  assert.equal(revision, `office-render-runtime-sha256-v1:${expected}`)
})

test(
  'preparation publishes a strict receipt last, reuses verified output, and verifies offline',
  { skip: process.platform === 'win32' },
  async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-'))
    const outputDirectory = join(parent, 'current')
    let installs = 0
    const installer = async (context) => {
      installs += 1
      return fixtureInstaller(context)
    }
    const first = await prepareOfficeRenderer({ outputDirectory, installer })
    assert.equal(first.reused, false)
    assert.equal(installs, 1)
    assert.deepEqual(Object.keys(first.receipt), [
      'schemaVersion',
      'providerId',
      'bundleVersion',
      'platform',
      'arch',
      'browser',
      'archive',
      'files',
      'bundleRevision'
    ])
    assert.equal(first.receipt.browser.version, '149.0.7827.55')
    assert.equal(first.receipt.browser.playwrightVersion, '1.61.1')
    assert.match(first.receipt.archive.sha256, /^[a-f0-9]{64}$/)
    assert.ok(first.receipt.files.length >= 2)
    assert.ok(first.receipt.files.every(({ path }) => path.startsWith('browser/')))
    assert.equal(
      first.receipt.files.some(({ path }) => path === 'component-receipt.json'),
      false
    )
    assert.deepEqual(
      first.receipt.files.map(({ path }) => path),
      [...first.receipt.files.map(({ path }) => path)].sort((left, right) =>
        Buffer.from(left).compare(Buffer.from(right))
      )
    )
    const receipt = JSON.parse(
      await readFile(join(outputDirectory, 'component-receipt.json'), 'utf8')
    )
    assert.equal(receipt.bundleRevision, first.receipt.bundleRevision)

    const second = await prepareOfficeRenderer({ outputDirectory, installer })
    assert.equal(second.reused, true)
    assert.equal(installs, 1)

    const offline = await prepareOfficeRenderer({
      outputDirectory,
      verifyOnly: true,
      installer: () => {
        throw new Error('verify mode attempted installation')
      }
    })
    assert.equal(offline.reused, true)
    assert.equal(offline.receipt.bundleRevision, first.receipt.bundleRevision)
  }
)

test(
  'failed replacement preserves the previous verified component and cleans staging',
  { skip: process.platform === 'win32' },
  async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-fault-'))
    const outputDirectory = join(parent, 'current')
    const initial = await prepareOfficeRenderer({ outputDirectory, installer: fixtureInstaller })
    await assert.rejects(
      prepareOfficeRenderer({
        outputDirectory,
        installer: fixtureInstaller,
        forceRebuild: true,
        hooks: {
          beforePublish() {
            throw new Error('injected-before-publish')
          }
        }
      }),
      /injected-before-publish/
    )
    const afterFault = await prepareOfficeRenderer({ outputDirectory, verifyOnly: true })
    assert.equal(afterFault.receipt.bundleRevision, initial.receipt.bundleRevision)
    assert.equal((await readdir(parent)).filter((name) => name.endsWith('.staging')).length, 0)
  }
)

test(
  'signing receipt refresh permits only declared frozen files to change',
  { skip: process.platform === 'win32' },
  async () => {
    const outputDirectory = join(
      await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-signing-')),
      'current'
    )
    const prepared = await prepareOfficeRenderer({ outputDirectory, installer: fixtureInstaller })
    const executablePath = join(outputDirectory, ...prepared.receipt.browser.executable.split('/'))
    await writeFile(executablePath, '#!/bin/sh\necho signed fixture\n', { mode: 0o755 })

    const refreshed = await refreshOfficeRendererReceiptAfterSigning({
      outputDirectory,
      originalReceipt: prepared.receipt,
      signedPaths: [prepared.receipt.browser.executable]
    })
    assert.notEqual(refreshed.bundleRevision, prepared.receipt.bundleRevision)
    assert.notEqual(
      refreshed.files.find(({ path }) => path === prepared.receipt.browser.executable).sha256,
      prepared.receipt.files.find(({ path }) => path === prepared.receipt.browser.executable).sha256
    )
    assert.deepEqual(await prepareOfficeRenderer({ outputDirectory, verifyOnly: true }), {
      outputDirectory,
      receipt: refreshed,
      reused: true
    })
  }
)

test(
  'signing receipt refresh rejects changes outside its exact allowlist',
  { skip: process.platform === 'win32' },
  async () => {
    const outputDirectory = join(
      await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-signing-boundary-')),
      'current'
    )
    const prepared = await prepareOfficeRenderer({ outputDirectory, installer: fixtureInstaller })
    const executablePath = join(outputDirectory, ...prepared.receipt.browser.executable.split('/'))
    const resource = prepared.receipt.files.find(({ path }) => path.endsWith('icudtl.dat')).path
    await writeFile(executablePath, '#!/bin/sh\necho signed fixture\n', { mode: 0o755 })
    await writeFile(join(outputDirectory, ...resource.split('/')), 'unexpected mutation\n')

    await assert.rejects(
      refreshOfficeRendererReceiptAfterSigning({
        outputDirectory,
        originalReceipt: prepared.receipt,
        signedPaths: [prepared.receipt.browser.executable]
      }),
      /outside the signing allowlist changed/
    )
    assert.deepEqual(
      JSON.parse(await readFile(join(outputDirectory, 'component-receipt.json'), 'utf8')),
      prepared.receipt
    )
  }
)

test(
  'preparation rejects symlinks and empty-directory pollution before publication',
  { skip: process.platform === 'win32' },
  async () => {
    const symlinkOutput = join(
      await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-link-')),
      'current'
    )
    await assert.rejects(
      prepareOfficeRenderer({
        outputDirectory: symlinkOutput,
        installer: (context) => fixtureInstaller(context, { symlink: true })
      }),
      /cannot contain symlink|forbidden symlink/
    )

    const emptyOutput = join(
      await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-empty-')),
      'current'
    )
    await assert.rejects(
      prepareOfficeRenderer({
        outputDirectory: emptyOutput,
        installer: (context) => fixtureInstaller(context, { emptyDirectory: true })
      }),
      /unexpected or empty directories/
    )
  }
)

test(
  'offline verification rejects added files and modified bytes',
  { skip: process.platform === 'win32' },
  async () => {
    const addedOutput = join(
      await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-added-')),
      'current'
    )
    await prepareOfficeRenderer({ outputDirectory: addedOutput, installer: fixtureInstaller })
    await writeFile(join(addedOutput, 'browser', 'unexpected.txt'), 'pollution\n')
    await assert.rejects(
      prepareOfficeRenderer({ outputDirectory: addedOutput, verifyOnly: true }),
      /do not match the frozen receipt/
    )

    const modifiedOutput = join(
      await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-modified-')),
      'current'
    )
    const prepared = await prepareOfficeRenderer({
      outputDirectory: modifiedOutput,
      installer: fixtureInstaller
    })
    const resource = prepared.receipt.files.find(({ path }) => path.endsWith('icudtl.dat')).path
    await writeFile(join(modifiedOutput, ...resource.split('/')), 'modified bytes\n')
    await assert.rejects(
      prepareOfficeRenderer({ outputDirectory: modifiedOutput, verifyOnly: true }),
      /do not match the frozen receipt/
    )
  }
)

test(
  'offline verification rejects a browser without a Unix execute bit',
  { skip: process.platform === 'win32' },
  async () => {
    const outputDirectory = join(
      await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-mode-')),
      'current'
    )
    const prepared = await prepareOfficeRenderer({ outputDirectory, installer: fixtureInstaller })
    const executable = join(outputDirectory, ...prepared.receipt.browser.executable.split('/'))
    await chmod(executable, 0o644)
    await assert.rejects(
      prepareOfficeRenderer({ outputDirectory, verifyOnly: true }),
      /must have a Unix execute bit/
    )
  }
)
