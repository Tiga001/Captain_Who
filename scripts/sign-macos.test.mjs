/* eslint-disable @typescript-eslint/explicit-function-return-type -- Test fixtures intentionally use compact JavaScript callbacks. */

import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import {
  APP_CODE_SIGN_IDENTIFIER,
  CORE_SERVER_CODE_SIGN_IDENTIFIER,
  OFFICE_RENDERER_CODE_SIGN_IDENTIFIERS,
  OFFICECLI_CODE_SIGN_IDENTIFIER,
  artifactRuntimeMacCodeSigningTargets,
  createMacSignOptions,
  frozenMachOCodeSignIdentifier,
  inspectFrozenMachOArchitectures,
  inspectOfficeRendererMachO,
  isCoreServerSigningTarget,
  officeCliMacCodeSigningTargets,
  officeRendererMacCodeSigningTargets,
  resolvePromiseSigningFunction,
  signArtifactRuntimeMachO,
  signOfficeCliMachO,
  signOfficeRendererMachO
} from './sign-macos.mjs'

const APP_PATH = '/build/mac-arm64/Captain Who.app'
const CORE_SERVER_PATH = join(APP_PATH, 'Contents', 'Resources', 'core-server')
const OFFICE_BROWSER_DIRECTORY = 'browser/chrome-headless-shell-mac-arm64'

function officeRendererReceipt() {
  return {
    arch: 'arm64',
    browser: { executable: `${OFFICE_BROWSER_DIRECTORY}/chrome-headless-shell` },
    files: Object.keys(OFFICE_RENDERER_CODE_SIGN_IDENTIFIERS).map((name) => ({
      path: `${OFFICE_BROWSER_DIRECTORY}/${name}`,
      size: 1,
      sha256: '0'.repeat(64)
    }))
  }
}

function developerIdDetails(identifier) {
  return `Identifier=${identifier}
CodeDirectory v=20500 size=512 flags=0x10000(runtime) hashes=10+7 location=embedded
Authority=Developer ID Application: Example Company (TEAM123456)
Authority=Developer ID Certification Authority
Authority=Apple Root CA
Timestamp=Jul 23, 2026 at 14:00:00
TeamIdentifier=TEAM123456`
}

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

test('managed native code-sign identifiers derive from the Captain Who application identity', () => {
  assert.equal(APP_CODE_SIGN_IDENTIFIER, 'io.github.tiga001.captainwho')
  assert.equal(CORE_SERVER_CODE_SIGN_IDENTIFIER, `${APP_CODE_SIGN_IDENTIFIER}.core-server`)
  assert.equal(OFFICECLI_CODE_SIGN_IDENTIFIER, `${APP_CODE_SIGN_IDENTIFIER}.officecli`)
  assert.equal(
    OFFICE_RENDERER_CODE_SIGN_IDENTIFIERS['chrome-headless-shell'],
    `${APP_CODE_SIGN_IDENTIFIER}.office-renderer.chrome-headless-shell`
  )
})

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
    'Captain Who Helper.app',
    'Contents',
    'MacOS',
    'Captain Who Helper'
  )
  assert.deepEqual(options.optionsForFile(electronHelper), {
    entitlements: '/build/entitlements.mac.plist',
    hardenedRuntime: false,
    additionalArguments: []
  })
})

test('Office renderer signing targets are the four exact frozen Chromium Mach-O paths', () => {
  const targets = officeRendererMacCodeSigningTargets(APP_PATH, officeRendererReceipt())
  assert.deepEqual(
    targets.map(({ relativePath, identifier }) => ({ relativePath, identifier })),
    Object.entries(OFFICE_RENDERER_CODE_SIGN_IDENTIFIERS).map(([name, identifier]) => ({
      relativePath: `${OFFICE_BROWSER_DIRECTORY}/${name}`,
      identifier
    }))
  )
  assert.equal(
    targets.every(({ path }) => path.startsWith(`${APP_PATH}/Contents/Resources/`)),
    true
  )
  assert.throws(
    () =>
      officeRendererMacCodeSigningTargets(APP_PATH, {
        ...officeRendererReceipt(),
        browser: { executable: 'browser/../../escaped/chrome-headless-shell' }
      }),
    /unexpected macOS Chromium executable path/
  )
  const missingLibrary = officeRendererReceipt()
  missingLibrary.files.pop()
  assert.throws(
    () => officeRendererMacCodeSigningTargets(APP_PATH, missingLibrary),
    /missing required macOS code/
  )
})

test('frozen component signing targets use stable identifiers, dependency order, and minimal JIT policy', () => {
  const targets = artifactRuntimeMacCodeSigningTargets([
    {
      relativePath: 'dependencies/python/bin/python3.12',
      path: '/component/dependencies/python/bin/python3.12'
    },
    {
      relativePath: 'dependencies/node/bin/node',
      path: '/component/dependencies/node/bin/node'
    },
    {
      relativePath: 'dependencies/python/lib/example.dylib',
      path: '/component/dependencies/python/lib/example.dylib'
    }
  ])
  assert.deepEqual(
    targets.map(({ relativePath }) => relativePath),
    [
      'dependencies/python/lib/example.dylib',
      'dependencies/node/bin/node',
      'dependencies/python/bin/python3.12'
    ]
  )
  assert.equal(new Set(targets.map(({ identifier }) => identifier)).size, targets.length)
  for (const target of targets) {
    assert.match(
      target.identifier,
      /^io\.github\.tiga001\.captainwho\.artifact-runtime\.[a-f0-9]{24}$/
    )
    assert.equal(target.identifier.includes(target.relativePath), false)
    if (target.relativePath === 'dependencies/node/bin/node') {
      assert.match(target.entitlements, /entitlements\.jit-runtime\.mac\.plist$/)
    } else {
      assert.equal(target.entitlements, undefined)
    }
  }
  assert.equal(
    frozenMachOCodeSignIdentifier('artifact-runtime', targets[0].relativePath),
    targets[0].identifier
  )

  const officeCli = officeCliMacCodeSigningTargets([
    { relativePath: 'officecli', path: '/component/officecli' }
  ])[0]
  assert.equal(officeCli.identifier, OFFICECLI_CODE_SIGN_IDENTIFIER)
  assert.match(officeCli.entitlements, /entitlements\.jit-runtime\.mac\.plist$/)
})

test('frozen Mach-O preflight accepts target-bearing universal binaries and rejects architecture drift', async () => {
  const targets = [
    { relativePath: 'thin', path: '/component/thin' },
    { relativePath: 'universal', path: '/component/universal' }
  ]
  const inspected = await inspectFrozenMachOArchitectures({
    targets,
    expectedArch: 'arm64',
    async run(_command, argumentsList) {
      return {
        stdout: argumentsList.at(-1).endsWith('universal') ? 'x86_64 arm64\n' : 'arm64\n'
      }
    }
  })
  assert.deepEqual(
    inspected.map(({ architectures }) => architectures),
    [['arm64'], ['x86_64', 'arm64']]
  )
  await assert.rejects(
    inspectFrozenMachOArchitectures({
      targets: [targets[0]],
      expectedArch: 'arm64',
      async run() {
        return { stdout: 'x86_64\n' }
      }
    }),
    /architecture mismatch/
  )
  await assert.rejects(
    inspectFrozenMachOArchitectures({
      targets: [targets[0]],
      expectedArch: 'arm64',
      async run() {
        return { stdout: 'arm64 ppc64\n' }
      }
    }),
    /architecture mismatch/
  )
})

test('Office renderer preflight discovers exactly four thin Mach-O files of the target arch', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-office-renderer-macho-'))
  const appPath = join(directory, 'Captain Who.app')
  const receipt = officeRendererReceipt()
  const targets = officeRendererMacCodeSigningTargets(appPath, receipt)
  const outputDirectory = join(appPath, 'Contents', 'Resources', 'components', 'office-renderer')
  const arm64Header = Buffer.from([0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0x00, 0x00, 0x01])
  for (const target of targets) {
    await mkdir(join(target.path, '..'), { recursive: true })
    await writeFile(target.path, arm64Header)
  }
  await writeFile(join(outputDirectory, 'component-receipt.json'), '{}\n')

  const discovered = await inspectOfficeRendererMachO({
    outputDirectory,
    targets,
    expectedArch: 'arm64'
  })
  assert.equal(discovered.length, 4)
  await writeFile(join(outputDirectory, 'browser', 'unexpected.dylib'), arm64Header)
  await assert.rejects(
    inspectOfficeRendererMachO({ outputDirectory, targets, expectedArch: 'arm64' }),
    /does not match its exact four-file allowlist/
  )
})

test('custom signer signs and verifies four Mach-O files before refreshing the packaged receipt', async () => {
  const calls = []
  const refreshes = []
  const receipt = officeRendererReceipt()
  const targets = officeRendererMacCodeSigningTargets(APP_PATH, receipt)
  const result = await signOfficeRendererMachO(signingConfiguration(), {
    async run(command, argumentsList, options) {
      calls.push({ command, argumentsList, options })
      if (argumentsList[0] === '--display') {
        const target = targets.find(({ path }) => path === argumentsList.at(-1))
        return {
          stdout: '',
          stderr: `Identifier=${target.identifier}
CodeDirectory v=20500 size=512 flags=0x10000(runtime) hashes=10+7 location=embedded
Authority=Developer ID Application: Example Company (TEAM123456)
Authority=Developer ID Certification Authority
Authority=Apple Root CA
Timestamp=Jul 23, 2026 at 14:00:00
TeamIdentifier=TEAM123456`
        }
      }
      return { stdout: '', stderr: '' }
    },
    async prepare(options) {
      assert.equal(options.verifyOnly, true)
      assert.equal(options.platform, 'darwin')
      return { outputDirectory: options.outputDirectory, receipt, reused: true }
    },
    async inspect(options) {
      assert.equal(options.expectedArch, 'arm64')
      assert.deepEqual(options.targets, targets)
    },
    async refreshReceipt(options) {
      refreshes.push(options)
      return { ...receipt, bundleRevision: 'refreshed' }
    }
  })

  assert.equal(calls.length, 12)
  for (let index = 0; index < calls.length; index += 3) {
    const signing = calls[index]
    const verification = calls[index + 1]
    const display = calls[index + 2]
    assert.equal(signing.command, '/usr/bin/codesign')
    assert.deepEqual(signing.argumentsList.slice(0, 7), [
      '--sign',
      signingConfiguration().identity,
      '--force',
      '--timestamp',
      '--options',
      'runtime',
      '--identifier'
    ])
    assert.deepEqual(verification.argumentsList.slice(0, 3), [
      '--verify',
      '--strict',
      '--verbose=2'
    ])
    assert.deepEqual(display.argumentsList.slice(0, 2), ['--display', '--verbose=4'])
  }
  assert.match(calls.at(-3).argumentsList.at(-1), /chrome-headless-shell$/)
  assert.deepEqual(calls.at(-3).argumentsList.slice(-3, -1), [
    '--entitlements',
    expectOfficeRendererEntitlements(calls.at(-3).argumentsList.at(-2))
  ])
  assert.equal(refreshes.length, 1)
  assert.deepEqual(
    refreshes[0].signedPaths,
    Object.keys(OFFICE_RENDERER_CODE_SIGN_IDENTIFIERS).map(
      (name) => `${OFFICE_BROWSER_DIRECTORY}/${name}`
    )
  )
  assert.equal(result.targets.length, 4)
  assert.equal(result.receipt.bundleRevision, 'refreshed')
})

test('custom signer signs every Artifact Runtime Mach-O before atomically refreshing its receipt', async () => {
  const rawTargets = [
    {
      relativePath: 'dependencies/node/bin/node',
      path: join(
        APP_PATH,
        'Contents',
        'Resources',
        'components',
        'artifact-runtime',
        'dependencies',
        'node',
        'bin',
        'node'
      )
    },
    {
      relativePath: 'dependencies/python/lib/example.dylib',
      path: join(
        APP_PATH,
        'Contents',
        'Resources',
        'components',
        'artifact-runtime',
        'dependencies',
        'python',
        'lib',
        'example.dylib'
      )
    }
  ]
  const calls = []
  const refreshes = []
  const receipt = { arch: process.arch, files: [] }
  const result = await signArtifactRuntimeMachO(signingConfiguration(), {
    async run(command, argumentsList, options) {
      calls.push({ command, argumentsList, options })
      if (argumentsList[0] === '--display') {
        const targets = artifactRuntimeMacCodeSigningTargets(rawTargets)
        const target = targets.find(({ path }) => path === argumentsList.at(-1))
        return { stdout: '', stderr: developerIdDetails(target.identifier) }
      }
      return { stdout: '', stderr: '' }
    },
    async prepare(options) {
      assert.equal(options.platform, 'darwin')
      assert.match(options.outputDirectory, /components\/artifact-runtime$/)
      return { receipt, targets: rawTargets }
    },
    async inspect(options) {
      assert.equal(options.expectedArch, process.arch)
      assert.deepEqual(
        options.targets.map(({ relativePath }) => relativePath),
        ['dependencies/python/lib/example.dylib', 'dependencies/node/bin/node']
      )
    },
    async refreshReceipt(options) {
      refreshes.push(options)
      return { ...receipt, bundleRevision: 'refreshed-artifact-runtime' }
    }
  })

  assert.equal(calls.length, 6)
  assert.match(calls[0].argumentsList.at(-1), /example\.dylib$/)
  assert.equal(calls[0].argumentsList.includes('--entitlements'), false)
  assert.match(calls[3].argumentsList.at(-1), /dependencies\/node\/bin\/node$/)
  assert.match(calls[3].argumentsList.at(-2), /entitlements\.jit-runtime\.mac\.plist$/)
  assert.deepEqual(refreshes[0].signedPaths, [
    'dependencies/python/lib/example.dylib',
    'dependencies/node/bin/node'
  ])
  assert.equal(result.receipt.bundleRevision, 'refreshed-artifact-runtime')
})

test('custom signer preserves OfficeCLI CoreCLR JIT permission and refreshes signed provenance', async () => {
  const target = {
    relativePath: 'officecli',
    path: join(APP_PATH, 'Contents', 'Resources', 'components', 'officecli', 'officecli')
  }
  const calls = []
  let refreshed = false
  const receipt = { arch: process.arch, files: [] }
  const result = await signOfficeCliMachO(signingConfiguration(), {
    async run(command, argumentsList, options) {
      calls.push({ command, argumentsList, options })
      if (argumentsList[0] === '--display') {
        return { stdout: '', stderr: developerIdDetails(OFFICECLI_CODE_SIGN_IDENTIFIER) }
      }
      return { stdout: '', stderr: '' }
    },
    async prepare() {
      return { receipt, targets: [target] }
    },
    async inspect({ targets }) {
      assert.equal(targets.length, 1)
    },
    async refreshReceipt(options) {
      refreshed = true
      assert.deepEqual(options.signedPaths, ['officecli'])
      return { ...receipt, bundleRevision: 'refreshed-officecli' }
    }
  })

  assert.equal(calls.length, 3)
  assert.deepEqual(calls[0].argumentsList.slice(-3), [
    '--entitlements',
    expectOfficeRendererEntitlements(calls[0].argumentsList.at(-2)),
    target.path
  ])
  assert.equal(refreshed, true)
  assert.equal(result.receipt.bundleRevision, 'refreshed-officecli')
})

function expectOfficeRendererEntitlements(path) {
  assert.match(path, /entitlements\.jit-runtime\.mac\.plist$/)
  return path
}

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
    () => createMacSignOptions(signingConfiguration({ app: '/build/Captain Who' })),
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
