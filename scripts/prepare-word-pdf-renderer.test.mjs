/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node test helpers are intentionally compact. */

import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { chmod, mkdtemp, mkdir, readFile, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import {
  computeWordPdfRendererRevision,
  downloadPinnedWordPdfRendererArchive,
  installLinuxDebArchive,
  installWindowsMsi,
  loadWordPdfRendererManifest,
  prepareWordPdfRenderer,
  selectWordPdfRendererTarget,
  validateWordPdfRendererManifest
} from './prepare-word-pdf-renderer.mjs'

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)))
const manifestPath = join(repositoryRoot, 'resources', 'word-pdf-renderer-manifest.json')

async function rawManifest() {
  return JSON.parse(await readFile(manifestPath, 'utf8'))
}

async function fixtureInstaller({ installRoot, target, manifest }) {
  const installed = join(installRoot, 'fixture')
  await writeInstalledLayout(installed, target, manifest.libreOffice.version)
  await symlink('fixture.txt', join(installed, 'libreoffice', 'share', 'fixture-link.txt'))
  return installed
}

async function writeInstalledLayout(installed, target, version = '26.2.4.2') {
  for (const [relative, content] of [
    [target.executable, `#!/bin/sh\necho 'LibreOffice ${version}'\n`],
    [target.license, 'MPL-2.0\n'],
    [target.notice, 'LibreOffice notices\n'],
    ['libreoffice/share/fixture.txt', 'fixture\n']
  ]) {
    const path = join(installed, ...relative.split('/'))
    await mkdir(dirname(path), { recursive: true })
    await writeFile(path, content, { mode: relative === target.executable ? 0o755 : 0o644 })
  }
}

async function writeNativeLibreOfficeLayout(officeRoot, target, version = '26.2.4.2') {
  const nativeTarget = Object.fromEntries(
    ['executable', 'license', 'notice'].map((field) => [
      field,
      target[field].slice('libreoffice/'.length)
    ])
  )
  await writeInstalledLayout(
    dirname(officeRoot),
    {
      ...nativeTarget,
      executable: `${basename(officeRoot)}/${nativeTarget.executable}`,
      license: `${basename(officeRoot)}/${nativeTarget.license}`,
      notice: `${basename(officeRoot)}/${nativeTarget.notice}`
    },
    version
  )
}

test('manifest freezes real LibreOffice archives for every supported desktop target', async () => {
  const manifest = await loadWordPdfRendererManifest(manifestPath)
  assert.equal(manifest.providerId, 'mycopilot.word-pdf-render-runtime')
  assert.equal(manifest.bundleVersion, '2026.08.1')
  assert.equal(manifest.libreOffice.version, '26.2.4.2')
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
      const target = selectWordPdfRendererTarget(manifest, platform, arch)
      assert.match(target.executable, /^libreoffice\//)
      assert.match(target.archive.url, /^https:\/\/downloadarchive\.documentfoundation\.org\//)
      assert.ok(target.archive.size > 200_000_000)
      assert.match(target.archive.sha256, /^[a-f0-9]{64}$/)
    }
  }
  assert.throws(() => selectWordPdfRendererTarget(manifest, 'freebsd', 'x64'), /not packaged/)
})

test('manifest rejects mutable versions, path escapes, and fake archive identities', async () => {
  const mutable = await rawManifest()
  mutable.libreOffice.version = 'latest'
  assert.throws(() => validateWordPdfRendererManifest(mutable), /must pin LibreOffice/)

  const escaped = await rawManifest()
  escaped.targets['darwin-arm64'].executable = '../soffice'
  assert.throws(() => validateWordPdfRendererManifest(escaped), /canonical POSIX-relative/)

  const fake = await rawManifest()
  fake.targets['darwin-arm64'].archive.url = 'https://example.com/libreoffice.dmg'
  assert.throws(() => validateWordPdfRendererManifest(fake), /exact LibreOffice archive URL/)

  const weak = await rawManifest()
  weak.targets['darwin-arm64'].archive.sha256 = 'latest'
  assert.throws(() => validateWordPdfRendererManifest(weak), /lowercase SHA-256/)
})

test('Linux DEB and Windows MSI installers normalize their native layouts', async () => {
  const manifest = await loadWordPdfRendererManifest(manifestPath)

  const linuxRoot = await mkdtemp(join(tmpdir(), 'word-pdf-linux-installer-'))
  const linuxTarget = selectWordPdfRendererTarget(manifest, 'linux', 'x64')
  const linuxCalls = []
  const linuxInstalled = await installLinuxDebArchive(
    linuxRoot,
    join(linuxRoot, 'libreoffice.tar.gz'),
    linuxTarget,
    {
      extract: async ({ cwd }) => {
        const debs = join(cwd, linuxTarget.payload, 'DEBS')
        await mkdir(debs, { recursive: true })
        await writeFile(join(debs, 'libreoffice.deb'), 'fixture')
      },
      run: async (executable, args) => {
        linuxCalls.push({ executable, args })
        const packageRoot = args[2]
        await writeNativeLibreOfficeLayout(join(packageRoot, 'opt', 'libreoffice26.2'), linuxTarget)
      }
    }
  )
  assert.deepEqual(
    linuxCalls.map(({ executable, args }) => [executable, args[0]]),
    [['dpkg-deb', '--extract']]
  )
  assert.equal(
    await readFile(join(linuxInstalled, ...linuxTarget.license.split('/')), 'utf8'),
    'MPL-2.0\n'
  )

  const windowsRoot = await mkdtemp(join(tmpdir(), 'word-pdf-windows-installer-'))
  const windowsTarget = selectWordPdfRendererTarget(manifest, 'win32', 'x64')
  const windowsCalls = []
  const windowsInstalled = await installWindowsMsi(
    windowsRoot,
    join(windowsRoot, 'libreoffice.msi'),
    windowsTarget,
    {
      run: async (executable, args) => {
        windowsCalls.push({ executable, args })
        const targetArgument = args.find((argument) => argument.startsWith('TARGETDIR='))
        const administrativeRoot = targetArgument.slice('TARGETDIR='.length)
        await writeNativeLibreOfficeLayout(
          join(administrativeRoot, 'Program Files', 'LibreOffice'),
          windowsTarget
        )
      }
    }
  )
  assert.deepEqual(
    windowsCalls.map(({ executable, args }) => [executable, args.slice(0, 3)]),
    [['msiexec.exe', ['/a', join(windowsRoot, 'libreoffice.msi'), '/qn']]]
  )
  assert.equal(
    await readFile(join(windowsInstalled, ...windowsTarget.notice.split('/')), 'utf8'),
    'LibreOffice notices\n'
  )
})

test('streaming download checks exact size and SHA-256 after HTTPS mirror redirects', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'word-pdf-download-'))
  const payload = Buffer.from('authenticated LibreOffice fixture')
  const archive = {
    url: 'https://downloadarchive.documentfoundation.org/libreoffice/old/26.2.4.2/fixture',
    size: payload.length,
    sha256: createHash('sha256').update(payload).digest('hex')
  }
  const fetchImpl = async () =>
    new Response(payload, {
      status: 200,
      headers: { 'content-length': String(payload.length) }
    })
  const output = join(directory, 'archive')
  await downloadPinnedWordPdfRendererArchive(archive, output, fetchImpl)
  assert.deepEqual(await readFile(output), payload)
  await assert.rejects(
    downloadPinnedWordPdfRendererArchive(
      { ...archive, sha256: '0'.repeat(64) },
      join(directory, 'bad'),
      fetchImpl
    ),
    /frozen size or SHA-256/
  )
})

test('canonical receipt revision excludes its own revision field', () => {
  const left = { z: 2, a: { y: true, x: ['v'] } }
  const right = { a: { x: ['v'], y: true }, z: 2 }
  const revision = computeWordPdfRendererRevision(left)
  assert.equal(revision, computeWordPdfRendererRevision(right))
  assert.equal(revision, computeWordPdfRendererRevision({ ...left, bundleRevision: 'ignored' }))
  assert.match(revision, /^word-pdf-render-runtime-sha256-v1:[a-f0-9]{64}$/)
})

test(
  'preparation publishes atomically, reuses a verified component, and rejects tampering',
  { skip: process.platform === 'win32' },
  async () => {
    const parent = await mkdtemp(join(tmpdir(), 'word-pdf-renderer-'))
    const outputDirectory = join(parent, 'current')
    let installs = 0
    const installer = async (context) => {
      installs += 1
      return fixtureInstaller(context)
    }
    const options = {
      outputDirectory,
      platform: 'darwin',
      arch: 'arm64',
      installer,
      probe: async (path) => {
        await chmod(path, 0o755)
      }
    }
    const first = await prepareWordPdfRenderer(options)
    assert.equal(first.reused, false)
    assert.equal(installs, 1)
    assert.equal(first.receipt.runtime.version, '26.2.4.2')
    assert.ok(first.receipt.files.length >= 4)
    assert.deepEqual(first.receipt.links, [
      { path: 'libreoffice/share/fixture-link.txt', target: 'fixture.txt' }
    ])
    assert.match(first.receipt.bundleRevision, /^word-pdf-render-runtime-sha256-v1:/)

    const second = await prepareWordPdfRenderer(options)
    assert.equal(second.reused, true)
    assert.equal(installs, 1)

    await writeFile(join(outputDirectory, 'libreoffice', 'share', 'fixture.txt'), 'changed\n')
    await assert.rejects(
      prepareWordPdfRenderer({
        outputDirectory,
        platform: 'darwin',
        arch: 'arm64',
        verifyOnly: true
      }),
      /do not match the frozen receipt/
    )
  }
)

test(
  'verification rejects a safe recorded symlink retargeted outside the component',
  { skip: process.platform === 'win32' },
  async () => {
    const parent = await mkdtemp(join(tmpdir(), 'word-pdf-renderer-link-'))
    const outputDirectory = join(parent, 'current')
    await prepareWordPdfRenderer({
      outputDirectory,
      platform: 'darwin',
      arch: 'arm64',
      installer: fixtureInstaller,
      probe: async (path) => chmod(path, 0o755)
    })
    const link = join(outputDirectory, 'libreoffice', 'share', 'fixture-link.txt')
    await import('node:fs/promises').then(({ rm }) => rm(link))
    await symlink('../../../../outside', link)
    await assert.rejects(
      prepareWordPdfRenderer({
        outputDirectory,
        platform: 'darwin',
        arch: 'arm64',
        verifyOnly: true
      }),
      /escapes its component root|Cannot resolve|ENOENT/
    )
  }
)
