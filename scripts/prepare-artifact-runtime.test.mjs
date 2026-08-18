/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node's test runner infers helper contracts. */

import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import {
  chmod,
  cp,
  mkdtemp,
  mkdir,
  readFile,
  readdir,
  rename,
  stat,
  symlink,
  writeFile
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import {
  createManagedNodeDependencyEvidence,
  loadArtifactRuntimeManifest,
  prepareArtifactRuntime,
  prepareArtifactRuntimeLegalEvidence,
  prepareManagedNodeDependencies,
  readPinnedZipMembers,
  selectArtifactRuntimeAssets,
  validateArtifactRuntimeDownloadUrl,
  validateArtifactRuntimeManifest
} from './prepare-artifact-runtime.mjs'

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)))
const manifestPath = join(repositoryRoot, 'resources', 'artifact-runtime-manifest.json')
const requirementsPath = join(
  repositoryRoot,
  'resources',
  'artifact-runtime-python-requirements.txt'
)
const builderPath = join(repositoryRoot, 'scripts', 'prepare-artifact-runtime.mjs')
const bootstrapPath = join(repositoryRoot, 'resources', 'artifact-runtime', 'node-bootstrap.mjs')
const loaderPath = join(repositoryRoot, 'resources', 'artifact-runtime', 'node-loader.mjs')
const presentationSdkPath = join(
  repositoryRoot,
  'resources',
  'artifact-runtime',
  'presentation-sdk.mjs'
)
const pdfCliPath = join(repositoryRoot, 'crates', 'core', 'src', 'command', 'pdf_runtime_cli.py')

async function rawManifest() {
  return JSON.parse(await readFile(manifestPath, 'utf8'))
}

function storedZip(entries) {
  const localRecords = []
  const centralRecords = []
  let localOffset = 0
  for (const entry of entries) {
    const name = Buffer.from(entry.name, 'utf8')
    const content = Buffer.from(entry.content)
    const flags = entry.flags ?? 0
    const declaredSize = entry.declaredSize ?? content.length
    const local = Buffer.alloc(30)
    local.writeUInt32LE(0x04034b50, 0)
    local.writeUInt16LE(20, 4)
    local.writeUInt16LE(flags, 6)
    local.writeUInt16LE(0, 8)
    local.writeUInt32LE(0, 10)
    local.writeUInt32LE(0, 14)
    local.writeUInt32LE(content.length, 18)
    local.writeUInt32LE(declaredSize, 22)
    local.writeUInt16LE(name.length, 26)
    local.writeUInt16LE(0, 28)
    localRecords.push(local, name, content)

    const central = Buffer.alloc(46)
    central.writeUInt32LE(0x02014b50, 0)
    central.writeUInt16LE(20, 4)
    central.writeUInt16LE(20, 6)
    central.writeUInt16LE(flags, 8)
    central.writeUInt16LE(0, 10)
    central.writeUInt32LE(0, 12)
    central.writeUInt32LE(0, 16)
    central.writeUInt32LE(content.length, 20)
    central.writeUInt32LE(declaredSize, 24)
    central.writeUInt16LE(name.length, 28)
    central.writeUInt16LE(0, 30)
    central.writeUInt16LE(0, 32)
    central.writeUInt16LE(0, 34)
    central.writeUInt16LE(0, 36)
    central.writeUInt32LE(0, 38)
    central.writeUInt32LE(localOffset, 42)
    centralRecords.push(central, name)
    localOffset += local.length + name.length + content.length
  }
  const local = Buffer.concat(localRecords)
  const central = Buffer.concat(centralRecords)
  const eocd = Buffer.alloc(22)
  eocd.writeUInt32LE(0x06054b50, 0)
  eocd.writeUInt16LE(entries.length, 8)
  eocd.writeUInt16LE(entries.length, 10)
  eocd.writeUInt32LE(central.length, 12)
  eocd.writeUInt32LE(local.length, 16)
  return Buffer.concat([local, central, eocd])
}

test('pinned ripgrep ZIP extraction reads only exact bounded members and rejects hazards', () => {
  const archive = storedZip([
    { name: 'ripgrep/rg.exe', content: 'binary' },
    { name: 'ripgrep/COPYING', content: 'license' },
    { name: '../outside', content: 'must not be selected' }
  ])
  const members = readPinnedZipMembers(archive, ['ripgrep/rg.exe', 'ripgrep/COPYING'])
  assert.equal(members.get('ripgrep/rg.exe').toString(), 'binary')
  assert.equal(members.get('ripgrep/COPYING').toString(), 'license')
  assert.equal(members.has('../outside'), false)

  assert.throws(
    () =>
      readPinnedZipMembers(
        storedZip([{ name: 'ripgrep/rg.exe', content: 'encrypted', flags: 1 }]),
        ['ripgrep/rg.exe']
      ),
    /encrypted or malformed/
  )
  assert.throws(
    () =>
      readPinnedZipMembers(
        storedZip([
          { name: 'ripgrep/rg.exe', content: 'one' },
          { name: 'ripgrep/rg.exe', content: 'two' }
        ]),
        ['ripgrep/rg.exe']
      ),
    /repeats required entry/
  )
  assert.throws(
    () =>
      readPinnedZipMembers(
        storedZip([{ name: 'ripgrep/rg.exe', content: 'small', declaredSize: 600 * 1024 * 1024 }]),
        ['ripgrep/rg.exe']
      ),
    /oversized or truncated/
  )
})

test('manifest pins runtime assets, PDF tools, and dependency versions for every desktop target', async () => {
  const manifest = await loadArtifactRuntimeManifest(manifestPath)
  assert.equal(manifest.schemaVersion, 4)
  assert.equal(manifest.bundleVersion, '2026.08.4')
  assert.equal(manifest.node.version, '22.23.1')
  assert.equal(manifest.python.version, '3.12.13')
  assert.deepEqual(
    manifest.node.dependencies.map(({ name, version }) => [name, version]),
    [
      ['docx', '9.6.1'],
      ['exceljs', '4.4.0'],
      ['pptxgenjs', '4.0.1']
    ]
  )
  assert.deepEqual(
    manifest.python.dependencies.map(({ name, version }) => [name, version]),
    [
      ['openpyxl', '3.1.5'],
      ['pdfplumber', '0.11.9'],
      ['pypdf', '6.15.0'],
      ['pypdfium2', '5.12.1'],
      ['python-docx', '1.2.0'],
      ['python-pptx', '1.0.2'],
      ['reportlab', '4.4.9'],
      ['xlsxwriter', '3.2.9']
    ]
  )
  assert.equal(manifest.tools.pdfCli.version, '1')
  assert.equal(manifest.tools.pdfCli.target, 'runtime/pdf-runtime-cli.py')
  assert.equal(manifest.tools.ripgrep.version, '15.1.0')
  assert.equal(manifest.tools.ripgrep.licenseFiles.length, 3)
  assert.equal(manifest.node.presentationSdk, 'runtime/presentation-sdk.mjs')
  assert.equal(
    manifest.buildInputs.presentationSdk.path,
    'resources/artifact-runtime/presentation-sdk.mjs'
  )
  assert.match(manifest.buildInputs.presentationSdk.sha256, /^(?!0{64}$)[a-f0-9]{64}$/)
  assert.equal(manifest.buildInputs.pptxgenjsPatch.path, 'patches/pptxgenjs@4.0.1.patch')
  assert.match(manifest.buildInputs.pptxgenjsPatch.sha256, /^(?!0{64}$)[a-f0-9]{64}$/)
  for (const platform of ['darwin', 'linux', 'win32']) {
    for (const arch of ['arm64', 'x64']) {
      const selected = selectArtifactRuntimeAssets(manifest, platform, arch)
      assert.match(selected.node.sha256, /^[a-f0-9]{64}$/)
      assert.match(selected.python.sha256, /^[a-f0-9]{64}$/)
      assert.match(selected.ripgrep.sha256, /^[a-f0-9]{64}$/)
      assert.ok(selected.node.size > 20_000_000)
      assert.ok(selected.python.size > 15_000_000)
      assert.ok(selected.ripgrep.size > 1_000_000)
      assert.equal(selected.ripgrep.format, platform === 'win32' ? 'zip' : 'tar.gz')
    }
  }
})

test('managed Python requirements freeze the reviewed dependency closure and binary-only install policy', async () => {
  const requirements = await readFile(requirementsPath, 'utf8')
  const records = requirements
    .replaceAll(/\\\r?\n\s*/g, ' ')
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith('#'))
  const expectedPins = [
    'cffi==2.1.1',
    'charset-normalizer==3.4.9',
    'cryptography==46.0.3',
    'et-xmlfile==2.0.0',
    'lxml==6.0.2',
    'openpyxl==3.1.5',
    'pdfminer-six==20251230',
    'pdfplumber==0.11.9',
    'pillow==12.2.0',
    'pycparser==3.0',
    'pypdf==6.15.0',
    'pypdfium2==5.12.1',
    'python-docx==1.2.0',
    'python-pptx==1.0.2',
    'reportlab==4.4.9',
    'typing-extensions==4.16.0',
    'xlsxwriter==3.2.9'
  ]
  assert.deepEqual(
    records.map((record) => record.slice(0, record.indexOf(' '))).sort(),
    expectedPins.sort()
  )
  assert.ok(records.every((record) => / --hash=sha256:[a-f0-9]{64}(?: |$)/.test(record)))
  assert.equal(
    records.find((record) => record.startsWith('pypdfium2==')).match(/--hash=/g).length,
    6
  )

  const builder = await readFile(builderPath, 'utf8')
  assert.match(builder, /'--require-hashes'/)
  assert.match(builder, /'--only-binary=:all:'/)
  assert.match(builder, /'--no-cache-dir'/)
  assert.match(builder, /'--no-compile'/)
})

test('manifest and download policy fail closed on mutable or foreign supply-chain inputs', async () => {
  const version = await rawManifest()
  version.node.version = 'latest'
  assert.throws(() => validateArtifactRuntimeManifest(version), /pinned to 22\.23\.1/)

  const digest = await rawManifest()
  digest.python.assets['darwin-arm64'].sha256 = '0'.repeat(63)
  assert.throws(() => validateArtifactRuntimeManifest(digest), /SHA-256/)

  assert.throws(() => validateArtifactRuntimeDownloadUrl('http://nodejs.org/node'), /HTTPS/)
  assert.throws(() => validateArtifactRuntimeDownloadUrl('https://example.com/node'), /allowlisted/)
  assert.throws(
    () => validateArtifactRuntimeDownloadUrl('https://user:secret@github.com/archive'),
    /credentials/
  )

  const staleBuildInput = await rawManifest()
  staleBuildInput.buildInputs.nodeBootstrap.sha256 = '0'.repeat(64)
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-artifact-manifest-'))
  const staleManifestPath = join(directory, 'artifact-runtime-manifest.json')
  await writeFile(staleManifestPath, `${JSON.stringify(staleBuildInput)}\n`)
  await assert.rejects(
    () => loadArtifactRuntimeManifest(staleManifestPath),
    /build input nodeBootstrap SHA-256 does not match/
  )

  const stalePptxGenPatch = await rawManifest()
  stalePptxGenPatch.buildInputs.pptxgenjsPatch.sha256 = '0'.repeat(64)
  const stalePptxGenPatchManifestPath = join(directory, 'stale-pptxgenjs-patch-manifest.json')
  await writeFile(stalePptxGenPatchManifestPath, `${JSON.stringify(stalePptxGenPatch)}\n`)
  await assert.rejects(
    () => loadArtifactRuntimeManifest(stalePptxGenPatchManifestPath),
    /build input pptxgenjsPatch SHA-256 does not match/
  )
})

function run(executable, args, { cwd, env } = {}) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(executable, args, {
      cwd,
      env: { ...process.env, ...env },
      shell: false,
      stdio: ['ignore', 'pipe', 'pipe']
    })
    const stdout = []
    const stderr = []
    child.stdout.on('data', (chunk) => stdout.push(chunk))
    child.stderr.on('data', (chunk) => stderr.push(chunk))
    child.once('error', rejectPromise)
    child.once('close', (code) => {
      const result = {
        code,
        stdout: Buffer.concat(stdout).toString('utf8'),
        stderr: Buffer.concat(stderr).toString('utf8')
      }
      if (code === 0) resolvePromise(result)
      else rejectPromise(new Error(`fixture process failed: ${result.stderr}`))
    })
  })
}

async function managedNodeFixture() {
  const manifest = await loadArtifactRuntimeManifest(manifestPath)
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-managed-node-'))
  await prepareManagedNodeDependencies(manifest, directory)
  const runtime = join(directory, 'runtime')
  await mkdir(runtime, { recursive: true })
  await cp(bootstrapPath, join(runtime, 'node-bootstrap.mjs'))
  await cp(loaderPath, join(runtime, 'node-loader.mjs'))
  await cp(presentationSdkPath, join(runtime, 'presentation-sdk.mjs'))
  return {
    directory,
    moduleRoot: join(directory, ...manifest.node.packageRoot.split('/')),
    bootstrap: join(runtime, 'node-bootstrap.mjs'),
    manifest
  }
}

async function managedNodeSupplyChainFixture() {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-node-supply-chain-'))
  const sourcePackageDirectory = join(directory, 'packages', 'artifact-runtime-node')
  const allowedPackageRoot = join(directory, 'node_modules')
  const directPackage = join(allowedPackageRoot, 'office-fixture')
  const transitivePackage = join(directPackage, 'node_modules', 'office-helper')
  await mkdir(sourcePackageDirectory, { recursive: true })
  await mkdir(transitivePackage, { recursive: true })
  const packageManifestPath = join(sourcePackageDirectory, 'package.json')
  await writeFile(
    packageManifestPath,
    `${JSON.stringify({
      name: '@mycopilot/test-artifact-runtime',
      private: true,
      dependencies: { 'office-fixture': '1.0.0' }
    })}\n`
  )
  await writeFile(
    join(directPackage, 'package.json'),
    `${JSON.stringify({
      name: 'office-fixture',
      version: '1.0.0',
      dependencies: { 'office-helper': '2.0.0' }
    })}\n`
  )
  await writeFile(join(directPackage, 'index.js'), 'module.exports = "clean"\n')
  await writeFile(
    join(transitivePackage, 'package.json'),
    `${JSON.stringify({ name: 'office-helper', version: '2.0.0' })}\n`
  )
  await writeFile(join(transitivePackage, 'index.js'), 'module.exports = "helper"\n')
  const lockfilePath = join(directory, 'pnpm-lock.yaml')
  await writeFile(
    lockfilePath,
    [
      "lockfileVersion: '9.0'",
      '',
      'packages:',
      '',
      '  office-fixture@1.0.0:',
      `    resolution: {integrity: sha512-${Buffer.alloc(64, 1).toString('base64')}}`,
      '',
      '  office-helper@2.0.0:',
      `    resolution: {integrity: sha512-${Buffer.alloc(64, 2).toString('base64')}}`,
      '',
      'snapshots:',
      ''
    ].join('\n')
  )
  const rootDependencies = [
    {
      name: 'office-fixture',
      version: '1.0.0',
      identityFile: 'dependencies/node/node_modules/office-fixture/package.json'
    }
  ]
  const evidence = await createManagedNodeDependencyEvidence({
    sourcePackageDirectory,
    packageManifestPath,
    lockfilePath,
    rootDependencies,
    allowedPackageRoot
  })
  return {
    allowedPackageRoot,
    directPackage,
    evidence,
    lockfilePath,
    manifest: {
      node: {
        packageRoot: 'dependencies/node/node_modules',
        dependencies: rootDependencies
      }
    },
    packageManifestPath,
    sourcePackageDirectory
  }
}

async function expectSupplyChainPreparationFailure(fixture, pattern) {
  const staging = await mkdtemp(join(tmpdir(), 'mycopilot-node-supply-chain-staging-'))
  await assert.rejects(
    prepareManagedNodeDependencies(fixture.manifest, staging, {
      allowedPackageRoot: fixture.allowedPackageRoot,
      evidence: fixture.evidence,
      lockfilePath: fixture.lockfilePath,
      sourcePackageDirectory: fixture.sourcePackageDirectory
    }),
    pattern
  )
}

test('managed Node acquisition rejects package-byte pollution against frozen evidence', async () => {
  const fixture = await managedNodeSupplyChainFixture()
  await writeFile(join(fixture.directPackage, 'polluted.js'), 'unexpected package content\n')
  await expectSupplyChainPreparationFailure(
    fixture,
    /do not match the frozen supply-chain evidence/
  )
})

test('managed Node acquisition rejects package symlinks without dereferencing them', async () => {
  const internal = await managedNodeSupplyChainFixture()
  await symlink('index.js', join(internal.directPackage, 'internal-link.js'))
  await expectSupplyChainPreparationFailure(internal, /contains a forbidden symlink/)

  const external = await managedNodeSupplyChainFixture()
  const outsideFile = join(external.allowedPackageRoot, '..', 'outside.js')
  await writeFile(outsideFile, 'outside boundary\n')
  await symlink(outsideFile, join(external.directPackage, 'external-link.js'))
  await expectSupplyChainPreparationFailure(external, /contains a forbidden symlink/)

  const escapedRoot = await managedNodeSupplyChainFixture()
  const outsidePackage = join(escapedRoot.allowedPackageRoot, '..', 'escaped-office-fixture')
  await rename(escapedRoot.directPackage, outsidePackage)
  await symlink(outsidePackage, escapedRoot.directPackage, 'dir')
  await expectSupplyChainPreparationFailure(escapedRoot, /must be a real, non-symlink directory/)
})

test('managed ESM bootstrap loads real Office packages and ignores a workspace shadow package', async () => {
  const fixture = await managedNodeFixture()
  const workspace = await mkdtemp(join(tmpdir(), 'mycopilot-artifact-workspace-'))
  const shadow = join(workspace, 'node_modules', 'exceljs')
  await mkdir(shadow, { recursive: true })
  await writeFile(
    join(shadow, 'package.json'),
    '{"name":"exceljs","version":"0.0.0","type":"module","exports":"./index.mjs"}\n'
  )
  await writeFile(join(shadow, 'index.mjs'), 'throw new Error("workspace shadow loaded")\n')
  const script = join(workspace, 'smoke.mjs')
  await writeFile(
    script,
    [
      "import fs from 'fs'",
      "import { createRequire } from 'node:module'",
      "import ExcelJS from 'exceljs'",
      "import { Document } from 'docx'",
      "import pptxgen from 'pptxgenjs'",
      'const require = createRequire(import.meta.url)',
      "const RequiredExcelJS = require('exceljs')",
      'console.log(typeof fs.readFile, typeof ExcelJS.Workbook, typeof RequiredExcelJS.Workbook, typeof Document, typeof pptxgen)'
    ].join('\n')
  )
  const result = await run(process.execPath, ['--import', fixture.bootstrap, script], {
    cwd: workspace,
    env: { MYCOPILOT_ARTIFACT_NODE_MODULES: fixture.moduleRoot }
  })
  assert.equal(result.stdout.trim(), 'function function function function function')
})

async function offlineComponentSource() {
  if (process.platform === 'win32') throw new Error('Unix-only fixture')
  const fixture = await managedNodeFixture()
  const { manifest, directory } = fixture
  const nodeExecutable = join(directory, ...manifest.node.executable.unix.split('/'))
  await mkdir(dirname(nodeExecutable), { recursive: true })
  const quotedNode = process.execPath.replaceAll("'", "'\\''")
  await writeFile(
    nodeExecutable,
    `#!/bin/sh\nif [ "$1" = "--version" ]; then echo v${manifest.node.version}; exit 0; fi\nexec '${quotedNode}' "$@"\n`
  )
  await chmod(nodeExecutable, 0o755)

  const pythonExecutable = join(directory, ...manifest.python.executable.unix.split('/'))
  await mkdir(dirname(pythonExecutable), { recursive: true })
  const pythonVersions = Object.fromEntries(
    manifest.python.dependencies.map(({ name, version }) => [name, version])
  )
  await writeFile(
    pythonExecutable,
    `#!/bin/sh\nif [ "$1" = "--version" ]; then echo 'Python ${manifest.python.version}'; else echo '${JSON.stringify(pythonVersions)}'; fi\n`
  )
  await chmod(pythonExecutable, 0o755)
  for (const dependency of manifest.python.dependencies) {
    const identity = join(directory, ...dependency.identityFile.split('/'))
    await mkdir(dirname(identity), { recursive: true })
    const reviewedLicenseMetadata = {
      pdfplumber: null,
      pypdfium2: 'BSD-3-Clause, Apache-2.0, dependency licenses',
      reportlab:
        'BSD license (see license.txt for details), Copyright (c) 2000-2025, ReportLab Inc.'
    }
    const declaredLicense = Object.hasOwn(reviewedLicenseMetadata, dependency.name)
      ? reviewedLicenseMetadata[dependency.name]
      : 'MIT'
    await writeFile(
      identity,
      [
        `Name: ${dependency.name}`,
        `Version: ${dependency.version}`,
        ...(declaredLicense === null ? [] : [`License: ${declaredLicense}`]),
        `Home-page: https://example.invalid/${dependency.name}`,
        ''
      ].join('\n')
    )
    if (dependency.name !== 'openpyxl') {
      const license = join(dirname(identity), 'licenses', 'LICENSE')
      await mkdir(dirname(license), { recursive: true })
      await writeFile(license, `fixture license for ${dependency.name}\n`)
    }
  }
  const ripgrepExecutable = join(directory, ...manifest.tools.ripgrep.executable.unix.split('/'))
  await mkdir(dirname(ripgrepExecutable), { recursive: true })
  await writeFile(
    ripgrepExecutable,
    `#!/bin/sh\nif [ "$1" = "--version" ]; then echo 'ripgrep ${manifest.tools.ripgrep.version}'; exit 0; fi\nexit 2\n`
  )
  await chmod(ripgrepExecutable, 0o755)
  const pdfCliTarget = join(directory, ...manifest.tools.pdfCli.target.split('/'))
  await mkdir(dirname(pdfCliTarget), { recursive: true })
  await cp(pdfCliPath, pdfCliTarget)
  for (const descriptor of manifest.tools.ripgrep.licenseFiles) {
    const target = join(directory, ...descriptor.target.split('/'))
    await mkdir(dirname(target), { recursive: true })
    await writeFile(target, `fixture ripgrep ${descriptor.source}\n`)
  }
  await cp(manifestPath, join(directory, 'runtime-manifest.json'))
  const nodeLicense = join(directory, ...manifest.node.licenseFile.target.split('/'))
  await mkdir(dirname(nodeLicense), { recursive: true })
  await writeFile(nodeLicense, 'fixture Node license\n')
  const pythonLicense = join(
    directory,
    ...manifest.python.runtimeHome.split('/'),
    'lib',
    `python${manifest.python.version.split('.').slice(0, 2).join('.')}`,
    'LICENSE.txt'
  )
  await mkdir(dirname(pythonLicense), { recursive: true })
  await writeFile(pythonLicense, 'fixture CPython license\n')
  await mkdir(join(directory, 'legal'), { recursive: true })
  await writeFile(join(directory, 'legal', 'NOTICE'), 'offline fixture\n')
  return { directory, manifest }
}

test(
  'offline source preparation publishes receipt last, is idempotent, and preserves old output on fault',
  { skip: process.platform === 'win32' },
  async () => {
    const source = await offlineComponentSource()
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-artifact-publish-'))
    const output = join(parent, 'current')
    const first = await prepareArtifactRuntime({
      manifestPath,
      sourceDirectory: source.directory,
      outputDirectory: output
    })
    assert.equal(first.reused, false)
    assert.match(first.receipt.bundleRevision, /^artifact-runtime-bundle-sha256-v1:[a-f0-9]{64}$/)
    assert.ok((await stat(join(output, 'component-receipt.json'))).isFile())
    assert.ok(
      first.receipt.runtimes.node.identityFiles.includes(
        'dependencies/node/node-package-evidence.json'
      )
    )
    assert.ok(first.receipt.runtimes.node.identityFiles.includes('runtime/presentation-sdk.mjs'))
    assert.ok((await stat(join(output, 'runtime', 'presentation-sdk.mjs'))).isFile())
    assert.equal(first.receipt.tools.pdfCli.path, 'runtime/pdf-runtime-cli.py')
    assert.deepEqual(first.receipt.tools.pdfCli.identityFiles, ['runtime/pdf-runtime-cli.py'])
    assert.equal(first.receipt.tools.ripgrep.version, '15.1.0')
    assert.ok(first.receipt.tools.ripgrep.identityFiles.includes('dependencies/tools/rg'))
    assert.ok(first.receipt.tools.ripgrep.identityFiles.includes('legal/ripgrep/LICENSE-MIT'))
    const supplyChainEvidencePath = 'dependencies/node/node-package-evidence.json'
    const supplyChainReceiptFile = first.receipt.files.find(
      ({ path }) => path === supplyChainEvidencePath
    )
    assert.ok(supplyChainReceiptFile)
    const supplyChainEvidence = JSON.parse(
      await readFile(join(output, ...supplyChainEvidencePath.split('/')), 'utf8')
    )
    assert.equal(supplyChainEvidence.schemaVersion, 1)
    assert.match(supplyChainEvidence.graphRevision, /^sha256:[a-f0-9]{64}$/)
    assert.equal(supplyChainEvidence.packages.length, 93)
    assert.ok(
      supplyChainEvidence.packages.every(
        ({ integrity, files }) =>
          /^sha(?:256|384|512)-[A-Za-z0-9+/]+={0,2}$/.test(integrity) && files.length > 0
      )
    )
    const legal = JSON.parse(await readFile(join(output, 'component-legal.json'), 'utf8'))
    assert.equal(legal.schemaVersion, 3)
    assert.deepEqual(
      legal.tools.map(({ name, version, licenseExpression }) => ({
        name,
        version,
        licenseExpression
      })),
      [{ name: 'ripgrep', version: '15.1.0', licenseExpression: 'MIT OR Unlicense' }]
    )
    assert.ok(legal.packages.node.some(({ name, direct }) => name === 'exceljs' && direct))
    assert.ok(legal.packages.node.some(({ name }) => name === 'buffers'))
    assert.equal(
      legal.packages.node.find(({ name }) => name === 'buffers').licenseExpression,
      'NOASSERTION'
    )
    assert.equal(
      legal.packages.node.some(({ name }) => name === 'https'),
      false
    )
    assert.deepEqual(
      legal.packages.python
        .filter(({ direct }) => direct)
        .map(({ name }) => name)
        .sort(),
      [
        'openpyxl',
        'pdfplumber',
        'pypdf',
        'pypdfium2',
        'python-docx',
        'python-pptx',
        'reportlab',
        'xlsxwriter'
      ]
    )
    assert.equal(
      legal.packages.python.find(({ name }) => name === 'pdfplumber').licenseExpression,
      'MIT'
    )
    assert.equal(
      legal.packages.python.find(({ name }) => name === 'pypdfium2').licenseExpression,
      'NOASSERTION'
    )
    for (const entry of [
      ...legal.runtimes,
      ...legal.tools,
      ...legal.packages.node,
      ...legal.packages.python
    ]) {
      for (const evidence of entry.evidence) {
        assert.ok((await stat(join(output, ...evidence.path.split('/')))).isFile())
      }
    }

    const second = await prepareArtifactRuntime({
      manifestPath,
      sourceDirectory: source.directory,
      outputDirectory: output
    })
    assert.equal(second.reused, true)
    assert.equal(second.receipt.bundleRevision, first.receipt.bundleRevision)

    await assert.rejects(
      prepareArtifactRuntime({
        manifestPath,
        sourceDirectory: source.directory,
        outputDirectory: output,
        forceRebuild: true,
        hooks: {
          beforePublish() {
            throw new Error('injected-before-publish')
          }
        }
      }),
      /injected-before-publish/
    )
    const afterFault = await prepareArtifactRuntime({
      manifestPath,
      sourceDirectory: source.directory,
      outputDirectory: output,
      verifyOnly: true
    })
    assert.equal(afterFault.receipt.bundleRevision, first.receipt.bundleRevision)

    const staleBootstrap = join(source.directory, ...source.manifest.node.bootstrap.split('/'))
    await writeFile(
      staleBootstrap,
      `${await readFile(staleBootstrap, 'utf8')}\n// stale but still executable build input\n`
    )
    await assert.rejects(
      prepareArtifactRuntime({
        manifestPath,
        sourceDirectory: source.directory,
        outputDirectory: output,
        forceRebuild: true
      }),
      /published managed Node bootstrap SHA-256 does not match/
    )
    const afterStaleInput = await prepareArtifactRuntime({
      manifestPath,
      outputDirectory: output,
      verifyOnly: true
    })
    assert.equal(afterStaleInput.receipt.bundleRevision, first.receipt.bundleRevision)

    const preparedRipgrep = join(output, 'dependencies', 'tools', 'rg')
    const ripgrepBytes = await readFile(preparedRipgrep)
    await chmod(preparedRipgrep, 0o644)
    await assert.rejects(
      prepareArtifactRuntime({ manifestPath, outputDirectory: output, verifyOnly: true }),
      /not an executable regular file/
    )
    await chmod(preparedRipgrep, 0o755)
    await prepareArtifactRuntime({ manifestPath, outputDirectory: output, verifyOnly: true })

    const preparedPresentationSdk = join(output, 'runtime', 'presentation-sdk.mjs')
    const presentationSdkBytes = await readFile(preparedPresentationSdk)
    await writeFile(
      preparedPresentationSdk,
      Buffer.concat([presentationSdkBytes, Buffer.from('\n// tampered editor facade\n')])
    )
    await assert.rejects(
      prepareArtifactRuntime({ manifestPath, outputDirectory: output, verifyOnly: true }),
      /component files do not match|SHA-256|receipt/
    )
    await writeFile(preparedPresentationSdk, presentationSdkBytes)
    await prepareArtifactRuntime({ manifestPath, outputDirectory: output, verifyOnly: true })

    await writeFile(preparedRipgrep, '#!/bin/sh\nexit 9\n')
    await chmod(preparedRipgrep, 0o755)
    await assert.rejects(
      prepareArtifactRuntime({ manifestPath, outputDirectory: output, verifyOnly: true }),
      /component files do not match|SHA-256|receipt/
    )
    await writeFile(preparedRipgrep, ripgrepBytes)
    await chmod(preparedRipgrep, 0o755)
    await prepareArtifactRuntime({ manifestPath, outputDirectory: output, verifyOnly: true })
    assert.equal((await readdir(parent)).filter((name) => name.endsWith('.staging')).length, 0)
  }
)

test(
  'offline source preparation rejects polluted Node packages and symlinks before publication',
  { skip: process.platform === 'win32' },
  async () => {
    const polluted = await offlineComponentSource()
    await writeFile(
      join(
        polluted.directory,
        ...polluted.manifest.node.packageRoot.split('/'),
        'exceljs',
        'polluted.js'
      ),
      'unexpected package content\n'
    )
    const pollutedOutput = join(
      await mkdtemp(join(tmpdir(), 'mycopilot-artifact-polluted-output-')),
      'current'
    )
    await assert.rejects(
      prepareArtifactRuntime({
        manifestPath,
        sourceDirectory: polluted.directory,
        outputDirectory: pollutedOutput
      }),
      /do not match the frozen supply-chain evidence/
    )

    const linked = await offlineComponentSource()
    const outside = join(dirname(linked.directory), 'outside-node-package-byte.js')
    await writeFile(outside, 'outside package boundary\n')
    await symlink(
      outside,
      join(
        linked.directory,
        ...linked.manifest.node.packageRoot.split('/'),
        'exceljs',
        'outside-link.js'
      )
    )
    const linkedOutput = join(
      await mkdtemp(join(tmpdir(), 'mycopilot-artifact-linked-output-')),
      'current'
    )
    await assert.rejects(
      prepareArtifactRuntime({
        manifestPath,
        sourceDirectory: linked.directory,
        outputDirectory: linkedOutput
      }),
      /offline source contains a forbidden symlink/
    )
  }
)

test('legal evidence generation fails closed for an unreviewed package without a license file', async () => {
  const source = await offlineComponentSource()
  const packageRoot = join(
    source.directory,
    ...source.manifest.node.packageRoot.split('/'),
    'unreviewed-license-fixture'
  )
  await mkdir(packageRoot, { recursive: true })
  await writeFile(
    join(packageRoot, 'package.json'),
    JSON.stringify({
      name: 'unreviewed-license-fixture',
      version: '1.0.0',
      license: 'MIT',
      repository: 'https://example.invalid/unreviewed-license-fixture'
    })
  )
  await assert.rejects(
    prepareArtifactRuntimeLegalEvidence(source.manifest, source.directory),
    /has no license file or exact reviewed exception/
  )
})

test('reviewed Python PDF license evidence fails closed when wheel metadata changes', async () => {
  const source = await offlineComponentSource()
  const dependency = source.manifest.python.dependencies.find(({ name }) => name === 'pypdfium2')
  const identity = join(source.directory, ...dependency.identityFile.split('/'))
  await writeFile(
    identity,
    [
      `Name: ${dependency.name}`,
      `Version: ${dependency.version}`,
      'License: BSD-3-Clause',
      'Home-page: https://example.invalid/pypdfium2',
      ''
    ].join('\n')
  )
  await assert.rejects(
    prepareArtifactRuntimeLegalEvidence(source.manifest, source.directory),
    /no longer matches its reviewed license metadata/
  )
})
