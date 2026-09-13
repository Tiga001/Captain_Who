import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import { Arch } from 'electron-builder'
import { parse as parseYaml } from 'yaml'
import {
  createUpdateBuildConfiguration,
  getMacUpdatePublishConfiguration,
  validateUpdateBuildTarget,
  validateUpdateUrl
} from './update-config.mjs'

const repositoryRoot = fileURLToPath(new URL('..', import.meta.url))
const fixtureUrl = 'https://updates.example.test/releases/macos/arm64/'

test('development configuration needs no source and never infers a publisher', () => {
  const configuration = createUpdateBuildConfiguration({ environment: {} })
  assert.equal(configuration.publish, null)
  assert.equal(getMacUpdatePublishConfiguration(configuration), null)
  assert.deepEqual(configuration.mac.target, ['dmg'])
  assert.equal(configuration.dmg.writeUpdateInfo, false)
})

test('update-enabled releases require an HTTPS source with no credential-bearing URL components', () => {
  assert.equal(
    validateUpdateUrl('https://updates.example.test/releases'),
    'https://updates.example.test/releases/'
  )
  assert.equal(validateUpdateUrl(fixtureUrl), fixtureUrl)
  for (const value of [
    'http://updates.example.test/releases/',
    'file:///tmp/updates',
    '/relative/updates',
    'https://user:secret@updates.example.test/',
    'https://user@updates.example.test/',
    'https://@updates.example.test/',
    'https://updates.example.test/?signature=secret',
    'https://updates.example.test/?',
    'https://updates.example.test/#fragment',
    'https://updates.example.test/#',
    ' https://updates.example.test/',
    'https://updates.example.test/\n',
    'https://updates.example.test\\releases',
    'not a URL',
    null
  ]) {
    assert.throws(() => validateUpdateUrl(value, { required: true }), /HTTPS directory URL/)
  }
  for (const value of [undefined, '']) {
    assert.throws(() => validateUpdateUrl(value, { required: true }), /is required/)
  }
  assert.throws(
    () => createUpdateBuildConfiguration({ environment: { CAPTAIN_WHO_REQUIRE_UPDATES: '1' } }),
    /is required/
  )
})

test('release configuration preserves signing and frozen component contracts and enables arm64 artifacts', () => {
  const base = parseYaml(readFileSync(new URL('../electron-builder.yml', import.meta.url), 'utf8'))
  const configuration = createUpdateBuildConfiguration({
    environment: { CAPTAIN_WHO_UPDATE_URL: fixtureUrl }
  })
  assert.deepEqual(configuration.mac.target, [
    { target: 'dmg', arch: ['arm64'] },
    { target: 'zip', arch: ['arm64'] }
  ])
  assert.deepEqual(configuration.mac.publish, {
    provider: 'generic',
    url: fixtureUrl,
    channel: 'latest',
    useMultipleRangeRequest: false
  })
  assert.equal(configuration.forceCodeSigning, true)
  assert.equal(configuration.dmg.writeUpdateInfo, true)
  assert.equal(configuration.mac.notarize, false)
  assert.equal(configuration.dmg.sign, true)
  for (const key of [
    'appId',
    'afterPack',
    'afterSign',
    'extraResources',
    'files',
    'win',
    'linux',
    'nsis'
  ]) {
    assert.deepEqual(configuration[key], base[key])
  }
  for (const key of [
    'sign',
    'signIgnore',
    'type',
    'hardenedRuntime',
    'strictVerify',
    'entitlementsInherit'
  ]) {
    assert.deepEqual(configuration.mac[key], base.mac[key])
  }
  assert.equal(configuration.publish, null)
  assert.equal(configuration.mac.artifactName, 'Captain-Who-${version}-${arch}.${ext}')
})

test('final configuration rejects alternate providers, injected credentials and target overrides', () => {
  const configuration = createUpdateBuildConfiguration({
    environment: { CAPTAIN_WHO_UPDATE_URL: fixtureUrl }
  })
  for (const modified of [
    { ...configuration, publish: { provider: 'github' } },
    { ...configuration, win: { ...configuration.win, publish: configuration.mac.publish } },
    { ...configuration, zip: { publish: configuration.mac.publish } },
    {
      ...configuration,
      mac: { ...configuration.mac, publish: { ...configuration.mac.publish, token: 'unexpected' } }
    },
    {
      ...configuration,
      mac: { ...configuration.mac, publish: { ...configuration.mac.publish, channel: 'beta' } }
    },
    { ...configuration, mac: { ...configuration.mac, target: ['dmg', 'zip'] } },
    { ...configuration, forceCodeSigning: false },
    { ...configuration, dmg: { ...configuration.dmg, writeUpdateInfo: false } }
  ]) {
    assert.throws(() => getMacUpdatePublishConfiguration(modified))
  }
  const context = {
    packager: { config: configuration },
    electronPlatformName: 'darwin',
    arch: Arch.arm64,
    targets: [{ name: 'dmg' }, { name: 'zip' }]
  }
  assert.equal(validateUpdateBuildTarget(context).url, fixtureUrl)
  for (const arch of [Arch.x64, Arch.universal]) {
    assert.throws(() => validateUpdateBuildTarget({ ...context, arch }), /macOS arm64 only/)
  }
  assert.throws(
    () => validateUpdateBuildTarget({ ...context, electronPlatformName: 'mas' }),
    /macOS arm64 only/
  )
  assert.throws(
    () => validateUpdateBuildTarget({ ...context, targets: [{ name: 'dmg' }] }),
    /both DMG and ZIP/
  )
  for (const electronPlatformName of ['win32', 'linux']) {
    assert.equal(validateUpdateBuildTarget({ ...context, electronPlatformName }), null)
  }
})

test('verification CLI fails before any packaging when release source is absent and does not echo credentials', () => {
  const environment = {
    ...process.env,
    CAPTAIN_WHO_UPDATE_URL: '',
    CAPTAIN_WHO_REQUIRE_UPDATES: ''
  }
  const missing = spawnSync(process.execPath, ['scripts/update-config.mjs', '--require-url'], {
    cwd: repositoryRoot,
    env: environment,
    encoding: 'utf8'
  })
  assert.equal(missing.status, 1)
  assert.match(missing.stderr, /CAPTAIN_WHO_UPDATE_URL is required/)
  const invalid = spawnSync(process.execPath, ['scripts/update-config.mjs', '--require-url'], {
    cwd: repositoryRoot,
    env: {
      ...environment,
      CAPTAIN_WHO_UPDATE_URL: 'https://user:do-not-print-this@updates.example.test/'
    },
    encoding: 'utf8'
  })
  assert.equal(invalid.status, 1)
  assert.equal(`${invalid.stdout}${invalid.stderr}`.includes('do-not-print-this'), false)
  const valid = spawnSync(process.execPath, ['scripts/update-config.mjs', '--require-url'], {
    cwd: repositoryRoot,
    env: { ...environment, CAPTAIN_WHO_UPDATE_URL: fixtureUrl },
    encoding: 'utf8'
  })
  assert.equal(valid.status, 0, valid.stderr)
  assert.match(valid.stdout, /DMG \+ ZIP/)
})

test('installed electron-builder loads and schema-validates the dynamic configuration without building', () => {
  const result = spawnSync(
    process.execPath,
    [
      '--input-type=module',
      '-e',
      `
    import { createRequire } from 'node:module';
    const require = createRequire(import.meta.resolve('electron-builder/package.json'));
    const { getConfig, validateConfiguration } = require('app-builder-lib/out/util/config/config');
    const configuration = await getConfig(process.cwd(), 'scripts/update-config.mjs', { forceCodeSigning: true });
    await validateConfiguration(configuration, { add() {} });
    if (configuration.mac.publish.url !== process.env.CAPTAIN_WHO_UPDATE_URL) throw new Error('Update source drift');
    if (configuration.afterSign !== './scripts/verify-packaged-app.mjs') throw new Error('Signature hook drift');
  `
    ],
    {
      cwd: repositoryRoot,
      env: { ...process.env, CAPTAIN_WHO_UPDATE_URL: fixtureUrl, CAPTAIN_WHO_REQUIRE_UPDATES: '1' },
      encoding: 'utf8'
    }
  )
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`)
})
