/* eslint-disable @typescript-eslint/explicit-function-return-type -- Node test helpers are runtime-validated JavaScript. */
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'

import { assertIgnoredRustTests, findIgnoredRustTests } from './check-ignored-rust-tests.mjs'

async function withFixture({ files, registry }, callback) {
  const repositoryRoot = await mkdtemp(join(tmpdir(), 'mycopilot-ignored-rust-tests-'))
  try {
    for (const [file, contents] of Object.entries(files)) {
      const absolutePath = join(repositoryRoot, file)
      await mkdir(dirname(absolutePath), { recursive: true })
      await writeFile(absolutePath, contents, 'utf8')
    }
    const registryPath = join(repositoryRoot, 'scripts/ignored-rust-tests.json')
    await mkdir(dirname(registryPath), { recursive: true })
    await writeFile(registryPath, JSON.stringify({ tests: registry }), 'utf8')
    await callback({ registryPath, repositoryRoot })
  } finally {
    await rm(repositoryRoot, { recursive: true, force: true })
  }
}

const validEntry = {
  file: 'crates/core/src/lib.rs',
  name: 'requires_fixture',
  owner: 'core-runtime',
  reason: 'Requires an external fixture.',
  status: 'manual'
}

test('finds ignored tests across intervening Rust attributes', async () => {
  await withFixture(
    {
      files: {
        'crates/core/src/lib.rs':
          '#[test]\n#[ignore = "external fixture"]\n#[cfg(target_os = "macos")]\nasync fn requires_fixture() {}\n'
      },
      registry: [validEntry]
    },
    ({ repositoryRoot }) => {
      assert.deepEqual(findIgnoredRustTests({ repositoryRoot }), [
        { file: 'crates/core/src/lib.rs', name: 'requires_fixture', line: 2 }
      ])
    }
  )
})

test('accepts a complete registry with a manual status or a runner', async () => {
  await withFixture(
    {
      files: {
        'crates/core/src/lib.rs': '#[test]\n#[ignore]\nfn requires_fixture() {}\n',
        'crates/core/src/second.rs': '#[test]\n#[ignore = "slow"]\nfn release_profile() {}\n'
      },
      registry: [
        validEntry,
        {
          file: 'crates/core/src/second.rs',
          name: 'release_profile',
          owner: 'release-engineering',
          reason: 'Runs in the release gate.',
          runner: 'pnpm test:release'
        }
      ]
    },
    (options) => {
      const analysis = assertIgnoredRustTests(options)
      assert.equal(analysis.sourceTests.length, 2)
    }
  )
})

test('rejects missing, stale, duplicate, and unowned registry entries', async () => {
  await withFixture(
    {
      files: {
        'crates/core/src/lib.rs': '#[test]\n#[ignore]\nfn requires_fixture() {}\n'
      },
      registry: [
        { ...validEntry, owner: '' },
        { ...validEntry },
        {
          file: 'crates/core/src/stale.rs',
          name: 'removed_test',
          owner: 'core-runtime',
          reason: 'No longer exists.',
          status: 'manual'
        }
      ]
    },
    (options) => {
      assert.throws(
        () => assertIgnoredRustTests(options),
        /registry entry 1: owner must be a non-empty string[\s\S]*registry entry duplicated: crates\/core\/src\/lib\.rs::requires_fixture[\s\S]*registry entry does not match an ignored source test: crates\/core\/src\/stale\.rs::removed_test/
      )
    }
  )
})

test('rejects an ignored source test omitted from the registry', async () => {
  await withFixture(
    {
      files: {
        'crates/core/src/lib.rs': '#[test]\n#[ignore]\nfn requires_fixture() {}\n'
      },
      registry: []
    },
    (options) => {
      assert.throws(
        () => assertIgnoredRustTests(options),
        /ignored source test is unregistered: crates\/core\/src\/lib\.rs::requires_fixture/
      )
    }
  )
})

test('requires either a runner or an explicit manual status', async () => {
  await withFixture(
    {
      files: {
        'crates/core/src/lib.rs': '#[test]\n#[ignore]\nfn requires_fixture() {}\n'
      },
      registry: [{ ...validEntry, status: undefined }]
    },
    (options) => {
      assert.throws(
        () => assertIgnoredRustTests(options),
        /specify exactly one of a non-empty runner or status "manual"/
      )
    }
  )
})
