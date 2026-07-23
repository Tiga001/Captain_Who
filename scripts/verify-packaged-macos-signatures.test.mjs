/* eslint-disable @typescript-eslint/explicit-function-return-type -- Test fixtures intentionally use compact JavaScript callbacks. */

import assert from 'node:assert/strict'
import { join } from 'node:path'
import test from 'node:test'

import {
  assertStableSignatureMetadata,
  packagedMacApplicationPaths,
  parseCodeSignatureMetadata,
  verifyPackagedMacSignatures
} from './verify-packaged-macos-signatures.mjs'

const CONTEXT = {
  appOutDir: '/build/mac-arm64',
  electronPlatformName: 'darwin',
  packager: { appInfo: { productFilename: 'MyCopilot' } }
}

const APP_DETAILS = `Executable=/build/MyCopilot.app/Contents/MacOS/MyCopilot
Identifier=com.mycopilot.next
Format=app bundle with Mach-O thin (arm64)
CodeDirectory v=20500 size=1024 flags=0x10000(runtime) hashes=20+7 location=embedded
Signature size=9000
Authority=Developer ID Application: Example Company (TEAM123456)
Authority=Developer ID Certification Authority
Authority=Apple Root CA
Timestamp=Jul 23, 2026 at 14:00:00
TeamIdentifier=TEAM123456`

const HELPER_DETAILS = `Executable=/build/MyCopilot.app/Contents/Resources/core-server
Identifier=com.mycopilot.next.core-server
Format=Mach-O thin (arm64)
CodeDirectory v=20500 size=512 flags=0x10000(runtime) hashes=10+7 location=embedded
Signature size=9000
Authority=Developer ID Application: Example Company (TEAM123456)
Authority=Developer ID Certification Authority
Authority=Apple Root CA
Timestamp=Jul 23, 2026 at 14:00:00
TeamIdentifier=TEAM123456`

const DESIGNATED_REQUIREMENT =
  'designated => identifier "com.mycopilot.next.core-server" and anchor apple generic and certificate leaf[subject.OU] = TEAM123456'

test('packaged application paths fail closed and locate core-server exactly', () => {
  assert.deepEqual(packagedMacApplicationPaths(CONTEXT), {
    app: join('/build/mac-arm64', 'MyCopilot.app'),
    coreServer: join('/build/mac-arm64', 'MyCopilot.app', 'Contents', 'Resources', 'core-server')
  })
  assert.throws(
    () => packagedMacApplicationPaths({ ...CONTEXT, electronPlatformName: 'mas' }),
    /requires a darwin package/
  )
  assert.throws(
    () =>
      packagedMacApplicationPaths({
        ...CONTEXT,
        packager: { appInfo: { productFilename: '../MyCopilot' } }
      }),
    /productFilename is required/
  )
})

test('signature metadata parser extracts stable identity properties', () => {
  assert.deepEqual(parseCodeSignatureMetadata(HELPER_DETAILS), {
    identifier: 'com.mycopilot.next.core-server',
    teamIdentifier: 'TEAM123456',
    authorities: [
      'Developer ID Application: Example Company (TEAM123456)',
      'Developer ID Certification Authority',
      'Apple Root CA'
    ],
    adHoc: false,
    runtime: true,
    timestamp: 'Jul 23, 2026 at 14:00:00'
  })
})

test('stable signature assertion accepts matching Developer ID signatures', () => {
  assert.doesNotThrow(() =>
    assertStableSignatureMetadata({
      appMetadata: parseCodeSignatureMetadata(APP_DETAILS),
      helperMetadata: parseCodeSignatureMetadata(HELPER_DETAILS),
      designatedRequirement: DESIGNATED_REQUIREMENT,
      helperEntitlements: '<?xml version="1.0"?><plist><dict/></plist>'
    })
  )
})

test('stable signature assertion rejects ad-hoc, mutable, mismatched, and over-entitled helpers', () => {
  const appMetadata = parseCodeSignatureMetadata(APP_DETAILS)
  const helperMetadata = parseCodeSignatureMetadata(HELPER_DETAILS)
  const base = {
    appMetadata,
    helperMetadata,
    designatedRequirement: DESIGNATED_REQUIREMENT,
    helperEntitlements: '<plist><dict/></plist>'
  }

  assert.throws(
    () =>
      assertStableSignatureMetadata({
        ...base,
        helperMetadata: { ...helperMetadata, adHoc: true, teamIdentifier: 'not set' }
      }),
    /unsigned or ad-hoc/
  )
  assert.throws(
    () =>
      assertStableSignatureMetadata({
        ...base,
        helperMetadata: { ...helperMetadata, teamIdentifier: 'OTHERTEAM1' }
      }),
    /Team IDs differ/
  )
  assert.throws(
    () =>
      assertStableSignatureMetadata({
        ...base,
        designatedRequirement: 'designated => cdhash H"0123456789"'
      }),
    /tied to a mutable cdhash/
  )
  assert.throws(
    () =>
      assertStableSignatureMetadata({
        ...base,
        helperEntitlements:
          '<key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>'
      }),
    /contains unexpected entitlement/
  )
})

test('packaged verifier executes strict checks and inspects the frozen helper identity', async () => {
  const calls = []
  const outputs = [APP_DETAILS, HELPER_DETAILS, DESIGNATED_REQUIREMENT, '<plist><dict/></plist>']
  const run = async (command, argumentsList, options) => {
    calls.push({ command, argumentsList, options })
    if (argumentsList[0] === '--verify') {
      return { stdout: '', stderr: '' }
    }
    return { stdout: '', stderr: outputs.shift() }
  }

  await verifyPackagedMacSignatures(CONTEXT, run)

  assert.equal(calls.length, 6)
  assert.deepEqual(calls[0].argumentsList.slice(0, 4), [
    '--verify',
    '--deep',
    '--strict',
    '--verbose=2'
  ])
  assert.deepEqual(calls[1].argumentsList.slice(0, 3), ['--verify', '--strict', '--verbose=2'])
  assert.deepEqual(calls[4].argumentsList.slice(0, 2), ['--display', '-r-'])
  assert.deepEqual(calls[5].argumentsList.slice(0, 3), ['--display', '--entitlements', '-'])
})
