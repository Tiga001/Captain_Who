/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node's test runner infers helper contracts. */

import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'

import {
  artifactRuntimePythonDependencies,
  artifactRuntimePythonLayout,
  validateArtifactRuntimeManifest
} from './artifact-runtime/contract.mjs'
import { prepareArtifactRuntimeLegalEvidence } from './artifact-runtime/legal-evidence.mjs'
import { pruneManagedPythonTestFixtures } from './artifact-runtime/python-runtime.mjs'
import { buildReceipt, validateFrozenArtifactRuntimeReceipt } from './artifact-runtime/receipt.mjs'

async function manifestFixture() {
  return validateArtifactRuntimeManifest(
    JSON.parse(
      await readFile(
        new URL('../resources/artifact-runtime-manifest.json', import.meta.url),
        'utf8'
      )
    )
  )
}

async function temporaryDirectory(t) {
  const directory = await mkdtemp(join(tmpdir(), 'captainwho-python-platform-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  return directory
}

async function writeFixture(root, relative, contents) {
  const path = join(root, ...relative.split('/'))
  await mkdir(dirname(path), { recursive: true })
  await writeFile(path, contents)
  return path
}

test('Git autocrlf checkouts preserve every frozen build input digest', async (t) => {
  const manifest = await manifestFixture()
  const repository = await temporaryDirectory(t)
  const runGit = (args, input) => {
    const result = spawnSync('git', ['-c', 'core.autocrlf=true', ...args], {
      cwd: repository,
      input,
      windowsHide: true,
      maxBuffer: 16 * 1024 * 1024
    })
    assert.equal(result.status, 0, result.error?.message ?? result.stderr?.toString())
    return result.stdout
  }
  runGit(['init', '--quiet'])
  await writeFile(
    join(repository, '.gitattributes'),
    await readFile(new URL('../.gitattributes', import.meta.url))
  )
  // Exercise Git's actual checkout filter in an isolated repository, without
  // changing the developer's Git configuration, index, or working tree.
  for (const [name, descriptor] of Object.entries(manifest.buildInputs)) {
    const bytes = await readFile(new URL(`../${descriptor.path}`, import.meta.url))
    const object = runGit(['hash-object', '-w', '--stdin'], bytes).toString().trim()
    const checkout = runGit(['cat-file', '--filters', `--path=${descriptor.path}`, object])
    assert.equal(
      createHash('sha256').update(checkout).digest('hex'),
      descriptor.sha256,
      `${name} must keep its frozen bytes when Git autocrlf is enabled`
    )
  }
})

for (const platform of ['win32', 'darwin', 'linux']) {
  const expectedSitePackages =
    platform === 'win32'
      ? 'dependencies/python/Lib/site-packages'
      : 'dependencies/python/lib/python3.12/site-packages'
  const expectedLicense =
    platform === 'win32'
      ? 'dependencies/python/LICENSE.txt'
      : 'dependencies/python/lib/python3.12/LICENSE.txt'

  test(`${platform}: receipt identities use the pinned Python installation layout`, async () => {
    const manifest = await manifestFixture()
    const before = JSON.stringify(manifest)
    const dependencies = artifactRuntimePythonDependencies(manifest, platform)
    assert.deepEqual(artifactRuntimePythonLayout(manifest, platform), {
      sitePackages: expectedSitePackages,
      licenseFile: expectedLicense
    })
    const receipt = buildReceipt(manifest, platform, 'x64', [
      { path: 'runtime-manifest.json', size: 1, sha256: 'a'.repeat(64) }
    ])
    assert.deepEqual(
      receipt.runtimes.python.dependencies,
      manifest.python.dependencies.map((dependency) => ({
        ...dependency,
        identityFile: `${expectedSitePackages}/${dependency.identityFile.split('/').slice(-2).join('/')}`
      }))
    )
    assert.deepEqual(receipt.runtimes.python.identityFiles, [
      manifest.python.executable[platform === 'win32' ? 'win32' : 'unix'],
      ...dependencies.map(({ identityFile }) => identityFile)
    ])
    assert.deepEqual(receipt.runtimes.node.dependencies, manifest.node.dependencies)
    assert.equal(JSON.stringify(manifest), before)
    if (platform !== 'win32') assert.equal(dependencies, manifest.python.dependencies)
    validateFrozenArtifactRuntimeReceipt(receipt, manifest, platform, 'x64')

    const tampered = structuredClone(receipt)
    tampered.runtimes.python.dependencies[0].identityFile = 'dependencies/python/foreign/METADATA'
    assert.throws(
      () => validateFrozenArtifactRuntimeReceipt(tampered, manifest, platform, 'x64'),
      /does not match the pinned manifest/
    )
  })

  test(`${platform}: pandas pruning removes only the target installation's test fixtures`, async (t) => {
    const manifest = await manifestFixture()
    const staging = await temporaryDirectory(t)
    const pandasRoot = `${expectedSitePackages}/pandas`
    const kept = await writeFixture(staging, `${pandasRoot}/__init__.py`, '# runtime package\n')
    await writeFixture(staging, `${pandasRoot}/tests/test_fixture.py`, '# removable fixture\n')
    const otherSitePackages =
      platform === 'win32'
        ? 'dependencies/python/lib/python3.12/site-packages'
        : 'dependencies/python/Lib/site-packages'
    const otherFixture = await writeFixture(
      staging,
      `${otherSitePackages}/pandas/tests/test_fixture.py`,
      '# unrelated layout\n'
    )
    await pruneManagedPythonTestFixtures(manifest, staging, platform)
    await assert.rejects(stat(join(staging, ...`${pandasRoot}/tests`.split('/'))), {
      code: 'ENOENT'
    })
    assert.equal(await readFile(kept, 'utf8'), '# runtime package\n')
    assert.equal(await readFile(otherFixture, 'utf8'), '# unrelated layout\n')
    await assert.rejects(
      pruneManagedPythonTestFixtures(manifest, staging, platform),
      /missing its expected removable test fixture tree/
    )
  })

  test(`${platform}: legal evidence copies CPython and wheel licenses from the target layout`, async (t) => {
    const pinned = await manifestFixture()
    const manifest = {
      ...pinned,
      node: { ...pinned.node, dependencies: [] },
      python: {
        ...pinned.python,
        dependencies: pinned.python.dependencies.filter(({ name }) => name === 'pypdf')
      }
    }
    const staging = await temporaryDirectory(t)
    await mkdir(join(staging, ...manifest.node.packageRoot.split('/')), { recursive: true })
    await writeFixture(staging, manifest.node.licenseFile.target, 'Node license fixture\n')
    const cpythonLicense = 'CPython license fixture\n'
    await writeFixture(staging, expectedLicense, cpythonLicense)
    const distributionRoot = `${expectedSitePackages}/pypdf-6.15.0.dist-info`
    await writeFixture(
      staging,
      `${distributionRoot}/METADATA`,
      'Name: pypdf\nVersion: 6.15.0\nLicense-Expression: BSD-3-Clause\nHome-page: https://github.com/py-pdf/pypdf\n'
    )
    await writeFixture(staging, `${distributionRoot}/licenses/LICENSE`, 'pypdf license fixture\n')
    const legal = await prepareArtifactRuntimeLegalEvidence(manifest, staging, platform)
    const evidence = legal.runtimes[1].evidence[0]
    assert.equal(await readFile(join(staging, ...evidence.path.split('/')), 'utf8'), cpythonLicense)
    assert.equal(evidence.sha256, createHash('sha256').update(cpythonLicense).digest('hex'))
    assert.equal(evidence.size, Buffer.byteLength(cpythonLicense))
    assert.equal(legal.packages.python[0].installPath, distributionRoot)
    assert.equal(legal.packages.python[0].direct, true)
  })
}

test('Windows path mapping rejects unknown Python layouts instead of guessing', async () => {
  const pinned = await manifestFixture()
  const manifest = {
    ...pinned,
    python: {
      ...pinned.python,
      dependencies: [{ ...pinned.python.dependencies[0], identityFile: 'foreign/numpy/METADATA' }]
    }
  }
  assert.throws(
    () => artifactRuntimePythonDependencies(manifest, 'win32'),
    /unexpected package layout/
  )
})
