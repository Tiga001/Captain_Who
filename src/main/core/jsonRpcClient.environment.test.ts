import { EventEmitter } from 'node:events'
import { resolve } from 'node:path'
import { PassThrough } from 'node:stream'
import type { ChildProcessWithoutNullStreams } from 'node:child_process'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const electronApp = vi.hoisted(() => ({
  getPath: vi.fn(),
  isPackaged: false
}))
const spawnProcess = vi.hoisted(() => vi.fn())

vi.mock('electron', () => ({ app: electronApp }))
vi.mock('node:child_process', async (importOriginal) => {
  const original = await importOriginal<typeof import('node:child_process')>()
  return { ...original, spawn: spawnProcess }
})

import { CoreJsonRpcClient } from './jsonRpcClient'

class FakeCoreProcess extends EventEmitter {
  readonly stderr = new PassThrough()
  readonly stdin = new PassThrough()
  readonly stdout = new PassThrough()
  readonly kill = vi.fn()
}

const originalResourcesPathDescriptor = Object.getOwnPropertyDescriptor(process, 'resourcesPath')
const originalAppDataRoot = process.env.MYCOPILOT_APP_DATA_ROOT
const originalStorageDatabase = process.env.MYCOPILOT_STORAGE_DB
const inheritedMixedCaseStorageKey = 'MyCopilot_Storage_Db'
const inheritedMixedCaseRootKey = 'mycopilot_app_data_root'
const originalMixedCaseStorageDatabase = process.env[inheritedMixedCaseStorageKey]
const originalMixedCaseAppDataRoot = process.env[inheritedMixedCaseRootKey]

function restoreEnvironmentVariable(name: string, value: string | undefined): void {
  if (value === undefined) {
    delete process.env[name]
  } else {
    process.env[name] = value
  }
}

function spawnedEnvironment(): NodeJS.ProcessEnv {
  const options = spawnProcess.mock.calls[0]?.[2]
  if (!options || typeof options !== 'object' || !('env' in options) || !options.env) {
    throw new Error('Expected core-server to be spawned with an environment')
  }
  return options.env as NodeJS.ProcessEnv
}

describe('CoreJsonRpcClient application data root', () => {
  beforeEach(() => {
    spawnProcess.mockReset()
    electronApp.getPath.mockReset()
    electronApp.isPackaged = false
    spawnProcess.mockImplementation(
      () => new FakeCoreProcess() as unknown as ChildProcessWithoutNullStreams
    )
  })

  afterEach(() => {
    restoreEnvironmentVariable('MYCOPILOT_APP_DATA_ROOT', originalAppDataRoot)
    restoreEnvironmentVariable('MYCOPILOT_STORAGE_DB', originalStorageDatabase)
    restoreEnvironmentVariable(inheritedMixedCaseStorageKey, originalMixedCaseStorageDatabase)
    restoreEnvironmentVariable(inheritedMixedCaseRootKey, originalMixedCaseAppDataRoot)
    if (originalResourcesPathDescriptor) {
      Object.defineProperty(process, 'resourcesPath', originalResourcesPathDescriptor)
    } else {
      Reflect.deleteProperty(process, 'resourcesPath')
    }
  })

  it.each([
    ['development', false],
    ['packaged', true]
  ] as const)(
    'injects the Host-owned root and removes a legacy database override in %s',
    (_label, isPackaged) => {
      const appDataRoot = resolve('fixtures', 'MyCopilot User Data')
      const inheritedAppDataRoot = resolve('fixtures', 'untrusted-parent-root')
      const inheritedStorageDatabase = resolve('fixtures', 'legacy-storage.sqlite')
      electronApp.isPackaged = isPackaged
      Object.defineProperty(process, 'resourcesPath', {
        configurable: true,
        value: resolve('fixtures', 'MyCopilot.app', 'Contents', 'Resources')
      })
      process.env.MYCOPILOT_APP_DATA_ROOT = inheritedAppDataRoot
      process.env.MYCOPILOT_STORAGE_DB = inheritedStorageDatabase

      const client = new CoreJsonRpcClient({ appDataRoot })

      expect(spawnProcess).not.toHaveBeenCalled()
      expect(electronApp.getPath).not.toHaveBeenCalled()

      client.start()

      expect(spawnedEnvironment().MYCOPILOT_APP_DATA_ROOT).toBe(appDataRoot)
      expect(spawnedEnvironment()).not.toHaveProperty('MYCOPILOT_STORAGE_DB')
      expect(process.env.MYCOPILOT_APP_DATA_ROOT).toBe(inheritedAppDataRoot)
      expect(process.env.MYCOPILOT_STORAGE_DB).toBe(inheritedStorageDatabase)
    }
  )

  it('removes case variants before injecting the canonical child environment', () => {
    const appDataRoot = resolve('fixtures', 'MyCopilot User Data')
    delete process.env.MYCOPILOT_APP_DATA_ROOT
    delete process.env.MYCOPILOT_STORAGE_DB
    process.env[inheritedMixedCaseRootKey] = resolve('fixtures', 'mixed-case-root')
    process.env[inheritedMixedCaseStorageKey] = resolve('fixtures', 'mixed-case-storage.sqlite')

    new CoreJsonRpcClient({ appDataRoot }).start()

    expect(spawnedEnvironment().MYCOPILOT_APP_DATA_ROOT).toBe(appDataRoot)
    expect(spawnedEnvironment()).not.toHaveProperty(inheritedMixedCaseRootKey)
    expect(spawnedEnvironment()).not.toHaveProperty(inheritedMixedCaseStorageKey)
  })

  it('freezes the Electron fallback at construction while leaving process spawning lazy', () => {
    const firstRoot = resolve('fixtures', 'first user data')
    const laterRoot = resolve('fixtures', 'later user data')
    electronApp.getPath.mockReturnValueOnce(firstRoot)

    const client = new CoreJsonRpcClient()

    expect(spawnProcess).not.toHaveBeenCalled()
    expect(electronApp.getPath).toHaveBeenCalledOnce()
    expect(electronApp.getPath).toHaveBeenCalledWith('userData')

    electronApp.getPath.mockReturnValue(laterRoot)
    client.start()
    client.start()

    expect(spawnProcess).toHaveBeenCalledOnce()
    expect(spawnedEnvironment().MYCOPILOT_APP_DATA_ROOT).toBe(firstRoot)
    expect(electronApp.getPath).toHaveBeenCalledOnce()
  })

  it('rejects a relative application data root before spawning Core', () => {
    expect(() => new CoreJsonRpcClient({ appDataRoot: 'relative/user-data' })).toThrow(
      'Core application data root must be an absolute path'
    )
    expect(spawnProcess).not.toHaveBeenCalled()
  })
})
