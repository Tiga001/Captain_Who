/* eslint-disable @typescript-eslint/explicit-function-return-type -- Test fixtures intentionally use compact JavaScript callbacks. */

import assert from 'node:assert/strict'
import { join } from 'node:path'
import test from 'node:test'

import {
  CORE_SERVER_CODE_SIGN_IDENTIFIER,
  createMacSignOptions,
  isCoreServerSigningTarget,
  resolvePromiseSigningFunction
} from './sign-macos.mjs'

const APP_PATH = '/build/mac-arm64/MyCopilot.app'
const CORE_SERVER_PATH = join(APP_PATH, 'Contents', 'Resources', 'core-server')

function signingConfiguration(overrides = {}) {
  return {
    app: APP_PATH,
    identity: '0123456789ABCDEF0123456789ABCDEF01234567',
    platform: 'darwin',
    strictVerify: false,
    optionsForFile(filePath) {
      return {
        entitlements: '/build/entitlements.mac.plist',
        hardenedRuntime: false,
        additionalArguments: filePath.endsWith('core-server') ? ['--preserve-metadata=flags'] : []
      }
    },
    ...overrides
  }
}

test('core-server signing target uses the exact packaged helper path', () => {
  assert.equal(isCoreServerSigningTarget(APP_PATH, CORE_SERVER_PATH), true)
  assert.equal(
    isCoreServerSigningTarget(
      APP_PATH,
      join(APP_PATH, 'Contents', 'Resources', 'nested', 'core-server')
    ),
    false
  )
  assert.equal(isCoreServerSigningTarget(APP_PATH, `${CORE_SERVER_PATH}-backup`), false)
})

test('custom signer preserves inherited policy and freezes the helper identifier', () => {
  const options = createMacSignOptions(signingConfiguration())

  assert.equal(options.strictVerify, true)

  const helperOptions = options.optionsForFile(CORE_SERVER_PATH)
  assert.equal(helperOptions.hardenedRuntime, true)
  assert.match(helperOptions.entitlements, /entitlements\.core-server\.mac\.plist$/)
  assert.deepEqual(helperOptions.additionalArguments, [
    '--preserve-metadata=flags',
    '--identifier',
    CORE_SERVER_CODE_SIGN_IDENTIFIER
  ])

  const electronHelper = join(
    APP_PATH,
    'Contents',
    'Frameworks',
    'MyCopilot Helper.app',
    'Contents',
    'MacOS',
    'MyCopilot Helper'
  )
  assert.deepEqual(options.optionsForFile(electronHelper), {
    entitlements: '/build/entitlements.mac.plist',
    hardenedRuntime: false,
    additionalArguments: []
  })
})

test('custom signer rejects unsigned, ad-hoc, and malformed configurations', () => {
  assert.throws(
    () => createMacSignOptions(signingConfiguration({ identity: undefined })),
    /real Developer ID Application identity/
  )
  assert.throws(
    () => createMacSignOptions(signingConfiguration({ identity: '-' })),
    /ad-hoc and unsigned identities are forbidden/
  )
  assert.throws(
    () => createMacSignOptions(signingConfiguration({ platform: 'mas' })),
    /requires darwin/
  )
  assert.throws(
    () => createMacSignOptions(signingConfiguration({ app: '/build/MyCopilot' })),
    /requires a macOS \.app path/
  )
})

test('custom signer rejects conflicting and asynchronous per-file policy', () => {
  const conflicting = createMacSignOptions(
    signingConfiguration({
      optionsForFile() {
        return { additionalArguments: [`--identifier=${CORE_SERVER_CODE_SIGN_IDENTIFIER}`] }
      }
    })
  )
  assert.throws(
    () => conflicting.optionsForFile(CORE_SERVER_PATH),
    /already contain a code-sign identifier/
  )

  const asynchronous = createMacSignOptions(
    signingConfiguration({
      optionsForFile() {
        return Promise.resolve({})
      }
    })
  )
  assert.throws(
    () => asynchronous.optionsForFile(CORE_SERVER_PATH),
    /must return an object or null synchronously/
  )
})

test('custom signer accepts only the Promise API and never the legacy callback API', async () => {
  let completed = false
  const promiseSigner = resolvePromiseSigningFunction({
    sign() {
      throw new Error('legacy callback API must not be selected')
    },
    async signAsync() {
      await Promise.resolve()
      completed = true
    }
  })

  await promiseSigner({})
  assert.equal(completed, true)
  assert.throws(
    () => resolvePromiseSigningFunction({ sign: () => undefined }),
    /Promise-based signing function/
  )
})
