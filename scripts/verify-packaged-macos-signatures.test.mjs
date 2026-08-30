/* eslint-disable @typescript-eslint/explicit-function-return-type -- Test fixtures intentionally use compact JavaScript callbacks. */

import assert from 'node:assert/strict'
import { join } from 'node:path'
import test from 'node:test'

import {
  OFFICE_RENDERER_CODE_SIGN_IDENTIFIERS,
  frozenMachOCodeSignIdentifier
} from './sign-macos.mjs'

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
const OFFICE_BROWSER_DIRECTORY = 'browser/chrome-headless-shell-mac-arm64'
const OFFICE_RENDERER_RECEIPT = {
  arch: 'arm64',
  browser: { executable: `${OFFICE_BROWSER_DIRECTORY}/chrome-headless-shell` },
  files: Object.keys(OFFICE_RENDERER_CODE_SIGN_IDENTIFIERS).map((name) => ({
    path: `${OFFICE_BROWSER_DIRECTORY}/${name}`,
    size: 1,
    sha256: '0'.repeat(64)
  }))
}

function officeRendererDetails(identifier, teamIdentifier = 'TEAM123456') {
  return `Executable=/build/MyCopilot.app/Contents/Resources/components/office-renderer/code
Identifier=${identifier}
Format=Mach-O thin (arm64)
CodeDirectory v=20500 size=512 flags=0x10000(runtime) hashes=10+7 location=embedded
Signature size=9000
Authority=Developer ID Application: Example Company (${teamIdentifier})
Authority=Developer ID Certification Authority
Authority=Apple Root CA
Timestamp=Jul 23, 2026 at 14:00:00
TeamIdentifier=${teamIdentifier}`
}

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

test('stable signature assertion binds all four Office renderer files to the application signer', () => {
  const officeRendererMetadata = Object.entries(OFFICE_RENDERER_CODE_SIGN_IDENTIFIERS).map(
    ([name, identifier]) => ({
      target: { relativePath: `${OFFICE_BROWSER_DIRECTORY}/${name}`, identifier },
      metadata: parseCodeSignatureMetadata(officeRendererDetails(identifier))
    })
  )
  const base = {
    appMetadata: parseCodeSignatureMetadata(APP_DETAILS),
    helperMetadata: parseCodeSignatureMetadata(HELPER_DETAILS),
    officeRendererMetadata,
    designatedRequirement: DESIGNATED_REQUIREMENT,
    helperEntitlements: '<plist><dict/></plist>'
  }
  assert.doesNotThrow(() => assertStableSignatureMetadata(base))
  assert.throws(
    () =>
      assertStableSignatureMetadata({
        ...base,
        officeRendererMetadata: [
          ...officeRendererMetadata.slice(0, -1),
          {
            ...officeRendererMetadata.at(-1),
            metadata: parseCodeSignatureMetadata(
              officeRendererDetails(officeRendererMetadata.at(-1).target.identifier, 'OTHERTEAM1')
            )
          }
        ]
      }),
    /not signed by the same Developer ID identity/
  )
})

test('stable signature assertion binds frozen runtimes and their exact minimal entitlements', () => {
  const relativePath = 'dependencies/node/bin/node'
  const identifier = frozenMachOCodeSignIdentifier('artifact-runtime', relativePath)
  const frozenComponentMetadata = [
    {
      component: 'Artifact Runtime',
      target: { relativePath, identifier },
      metadata: parseCodeSignatureMetadata(officeRendererDetails(identifier)),
      entitlements: '<plist><dict><key>com.apple.security.cs.allow-jit</key><true/></dict></plist>',
      expectedEntitlements: ['com.apple.security.cs.allow-jit']
    }
  ]
  const base = {
    appMetadata: parseCodeSignatureMetadata(APP_DETAILS),
    helperMetadata: parseCodeSignatureMetadata(HELPER_DETAILS),
    frozenComponentMetadata,
    designatedRequirement: DESIGNATED_REQUIREMENT,
    helperEntitlements: '<plist><dict/></plist>'
  }
  assert.doesNotThrow(() => assertStableSignatureMetadata(base))
  assert.throws(
    () =>
      assertStableSignatureMetadata({
        ...base,
        frozenComponentMetadata: [
          {
            ...frozenComponentMetadata[0],
            entitlements: '<key>com.apple.security.cs.disable-library-validation</key><true/>'
          }
        ]
      }),
    /unexpected entitlements/
  )
  assert.throws(
    () =>
      assertStableSignatureMetadata({
        ...base,
        frozenComponentMetadata: [
          {
            ...frozenComponentMetadata[0],
            entitlements: '<key>com.apple.security.cs.allow-jit</key><false/>'
          }
        ]
      }),
    /unexpected entitlements/
  )
  assert.throws(
    () =>
      assertStableSignatureMetadata({
        ...base,
        frozenComponentMetadata: [
          {
            ...frozenComponentMetadata[0],
            metadata: parseCodeSignatureMetadata(officeRendererDetails(identifier, 'OTHERTEAM1'))
          }
        ]
      }),
    /not signed by the same Developer ID identity/
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
  const run = async (command, argumentsList, options) => {
    calls.push({ command, argumentsList, options })
    if (argumentsList[0] === '--verify') {
      return { stdout: '', stderr: '' }
    }
    if (argumentsList[1] === '-r-') {
      return { stdout: '', stderr: DESIGNATED_REQUIREMENT }
    }
    if (argumentsList[1] === '--entitlements') {
      return { stdout: '', stderr: '<plist><dict/></plist>' }
    }
    const path = argumentsList.at(-1)
    if (path.endsWith('MyCopilot.app')) return { stdout: '', stderr: APP_DETAILS }
    if (path.endsWith('core-server')) return { stdout: '', stderr: HELPER_DETAILS }
    const entry = Object.entries(OFFICE_RENDERER_CODE_SIGN_IDENTIFIERS).find(([name]) =>
      path.endsWith(name)
    )
    return { stdout: '', stderr: officeRendererDetails(entry[1]) }
  }

  await verifyPackagedMacSignatures(
    CONTEXT,
    run,
    { officeRendererReceipt: OFFICE_RENDERER_RECEIPT, frozenComponents: {} },
    {
      async resolveFrozenTargets() {
        return []
      }
    }
  )

  assert.equal(calls.length, 14)
  assert.deepEqual(calls[0].argumentsList.slice(0, 4), [
    '--verify',
    '--deep',
    '--strict',
    '--verbose=2'
  ])
  assert.deepEqual(calls[1].argumentsList.slice(0, 3), ['--verify', '--strict', '--verbose=2'])
  assert.equal(
    calls.slice(2, 6).every(({ argumentsList }) => argumentsList[0] === '--verify'),
    true
  )
  assert.equal(
    calls.slice(8, 12).every(({ argumentsList }) => argumentsList[0] === '--display'),
    true
  )
  assert.deepEqual(calls[12].argumentsList.slice(0, 2), ['--display', '-r-'])
  assert.deepEqual(calls[13].argumentsList.slice(0, 4), [
    '--display',
    '--entitlements',
    '-',
    '--xml'
  ])
  assert.equal(
    calls.every(({ command }) => command === '/usr/bin/codesign'),
    true
  )
})

test('packaged verifier fails closed when frozen component verification is omitted', async () => {
  await assert.rejects(
    verifyPackagedMacSignatures(CONTEXT, async () => ({ stdout: '', stderr: '' }), {
      officeRendererReceipt: OFFICE_RENDERER_RECEIPT
    }),
    /frozen component verification result is required/
  )
})
