import assert from 'node:assert/strict'
import test from 'node:test'

import {
  buildDevelopmentStorageResetCommand,
  readDevelopmentElectronAppName,
  resolveElectronAppDataRoot
} from './reset-dev-storage-command.mjs'

test('uses the workspace package identity before resolving the authoritative Electron root', () => {
  const configuredNames = []
  const requestedPaths = []
  const createdDirectories = []
  const canonicalizedPaths = []
  const app = {
    setName(name) {
      configuredNames.push(name)
    },
    getPath(name) {
      requestedPaths.push(name)
      assert.deepEqual(configuredNames, ['captain-who'])
      return '/electron-owned/captain-who'
    }
  }

  const root = resolveElectronAppDataRoot(
    app,
    'captain-who',
    (path) => {
      canonicalizedPaths.push(path)
      return '/canonical/captain-who'
    },
    (path, options) => {
      createdDirectories.push([path, options])
    }
  )

  assert.equal(root, '/canonical/captain-who')
  assert.deepEqual(configuredNames, ['captain-who'])
  assert.deepEqual(requestedPaths, ['userData'])
  assert.deepEqual(createdDirectories, [
    ['/electron-owned/captain-who', { recursive: true, mode: 0o700 }]
  ])
  assert.deepEqual(canonicalizedPaths, ['/electron-owned/captain-who'])
})

test('reads the development Electron identity from workspace package metadata', () => {
  assert.equal(readDevelopmentElectronAppName(), 'captain-who')
})

test('passes the exact Electron root and keeps reset dry-run unless explicitly confirmed', () => {
  const command = buildDevelopmentStorageResetCommand('/electron-owned/User Data', [], '/workspace')

  assert.equal(command.executable, 'cargo')
  assert.equal(command.cwd, '/workspace')
  assert.deepEqual(command.arguments.slice(-2), ['--app-data-root', '/electron-owned/User Data'])
  assert.equal(command.arguments.includes('--confirm-reset'), false)
})

test('forwards only an explicit confirmation after the authoritative root', () => {
  const command = buildDevelopmentStorageResetCommand(
    '/electron-owned/User Data',
    ['--', '--confirm-reset'],
    '/workspace'
  )

  assert.deepEqual(command.arguments.slice(-3), [
    '--app-data-root',
    '/electron-owned/User Data',
    '--confirm-reset'
  ])
})

test('rejects a relative root before launching Rust', () => {
  assert.throws(
    () => buildDevelopmentStorageResetCommand('relative/root', [], '/workspace'),
    /must be absolute/
  )
})

test('rejects unsupported or repeated forwarded arguments', () => {
  assert.throws(
    () => buildDevelopmentStorageResetCommand('/root', ['--force'], '/workspace'),
    /Only one optional/
  )
  assert.throws(
    () =>
      buildDevelopmentStorageResetCommand(
        '/root',
        ['--confirm-reset', '--confirm-reset'],
        '/workspace'
      ),
    /Only one optional/
  )
})
