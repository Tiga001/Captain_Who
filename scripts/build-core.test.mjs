import assert from 'node:assert/strict'
import test from 'node:test'

import { createCargoReleaseEnvironment } from './build-core.mjs'

const separator = '\u001f'

test('release Rust flags remap workspace and toolchain homes without fixed user identities', () => {
  const environment = createCargoReleaseEnvironment({
    environment: {},
    repositoryRoot: '/Users/release-builder/project',
    userHome: '/Users/release-builder'
  })
  const flags = environment.CARGO_ENCODED_RUSTFLAGS.split(separator)

  assert.deepEqual(flags, [
    '--remap-path-prefix',
    '/Users/release-builder/project=workspace',
    '--remap-path-prefix',
    '/Users/release-builder/.cargo=cargo-home',
    '--remap-path-prefix',
    '/Users/release-builder/.rustup=rustup-home'
  ])
})

test('release Rust flags preserve encoded caller flags and explicit toolchain homes', () => {
  const environment = createCargoReleaseEnvironment({
    environment: {
      CARGO_HOME: '/opt/cargo',
      RUSTUP_HOME: '/opt/rustup',
      CARGO_ENCODED_RUSTFLAGS: ['-C', 'target-cpu=apple-m1'].join(separator)
    },
    repositoryRoot: '/work/project',
    userHome: '/Users/unused'
  })

  assert.deepEqual(environment.CARGO_ENCODED_RUSTFLAGS.split(separator), [
    '-C',
    'target-cpu=apple-m1',
    '--remap-path-prefix',
    '/work/project=workspace',
    '--remap-path-prefix',
    '/opt/cargo=cargo-home',
    '--remap-path-prefix',
    '/opt/rustup=rustup-home'
  ])
})

test('release Rust flags reject ambiguous plain RUSTFLAGS instead of silently dropping them', () => {
  assert.throws(
    () =>
      createCargoReleaseEnvironment({
        environment: { RUSTFLAGS: '-C target-cpu=native' },
        repositoryRoot: '/work/project',
        userHome: '/Users/release-builder'
      }),
    /CARGO_ENCODED_RUSTFLAGS/
  )
})
