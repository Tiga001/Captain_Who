import { EventEmitter } from 'node:events'
import { resolve } from 'node:path'
import { PassThrough } from 'node:stream'
import type { ChildProcessWithoutNullStreams } from 'node:child_process'
import { NOTIFICATION_BATCHES_CLAIM_METHOD } from '@mycopilot/protocol'
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

import { CoreJsonRpcClient, CoreJsonRpcError } from './jsonRpcClient'

class FakeCoreProcess extends EventEmitter {
  pid: number | undefined = 12345
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
        value: resolve('fixtures', 'Captain Who.app', 'Contents', 'Resources')
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

  it('restarts Core after an unexpected exit while request admission remains open', async () => {
    const client = new CoreJsonRpcClient({ appDataRoot: resolve('fixtures', 'active-restart') })
    client.start()
    const first = spawnProcess.mock.results[0]?.value as FakeCoreProcess
    first.emit('exit', 1, null)

    const response = client.request<{ alive: boolean }>('core.ping', {})
    const second = spawnProcess.mock.results[1]?.value as FakeCoreProcess
    second.stdout.write('{"jsonrpc":"2.0","id":1,"result":{"alive":true}}\n')

    await expect(response).resolves.toEqual({ alive: true })
    expect(spawnProcess).toHaveBeenCalledTimes(2)
    client.stop()
  })

  it('rejects pending requests when Core closes stdin without raising an uncaught EPIPE', async () => {
    const client = new CoreJsonRpcClient({ appDataRoot: resolve('fixtures', 'stdin-failure') })
    client.start()
    const child = spawnProcess.mock.results[0]?.value as FakeCoreProcess
    const response = client.request<{ alive: boolean }>('core.ping', {})
    const error = new Error('write EPIPE')

    child.stdin.emit('error', error)

    await expect(response).rejects.toBe(error)
    expect(client.isRunning()).toBe(true)
    await expect(client.request('core.ping')).rejects.toBe(error)
    expect(spawnProcess).toHaveBeenCalledOnce()
    child.emit('exit', 1, null)
    expect(client.isRunning()).toBe(false)
  })

  it('never lazily restarts Core after explicit shutdown admission closes', async () => {
    const client = new CoreJsonRpcClient({ appDataRoot: resolve('fixtures', 'shutdown-fence') })
    client.start()
    client.beginShutdown()
    client.stop()

    expect(() => client.start()).toThrow('core-server request admission is closed')
    await expect(client.request(NOTIFICATION_BATCHES_CLAIM_METHOD, {})).rejects.toThrow(
      'core-server request admission is closed'
    )
    expect(spawnProcess).toHaveBeenCalledOnce()
  })

  it('fences an overloaded connection until actual exit and ignores its late notifications', async () => {
    const log = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const client = new CoreJsonRpcClient({ appDataRoot: resolve('fixtures', 'outbound-overload') })
    const received = vi.fn()
    client.onNotification('agent.event', received)
    const firstRequest = client.request('core.ping')
    const secondRequest = client.request('storage.loadConversations')
    const first = spawnProcess.mock.results[0]?.value as FakeCoreProcess
    const error = {
      code: -32002,
      message: 'Outbound queue capacity exceeded; this connection is closing',
      data: { code: 'outbound_overloaded', retryable: true }
    }
    const expectations = [firstRequest, secondRequest].map((request) =>
      expect(request).rejects.toEqual(new CoreJsonRpcError(error))
    )
    first.stdout.write('{"jsonrpc":"2.0","method":"agent.event","params":{"delta":"accepted"}}\n')
    first.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id: null, error })}\n`)
    await Promise.all(expectations)
    expect(received).toHaveBeenCalledExactlyOnceWith({ delta: 'accepted' })
    first.stdout.write('{"jsonrpc":"2.0","method":"agent.event","params":{"delta":"late"}}\n')
    first.stdin.emit('error', new Error('write EPIPE'))
    await expect(client.request('core.ping')).rejects.toMatchObject({ code: -32002 })
    expect(spawnProcess).toHaveBeenCalledOnce()
    first.emit('exit', 1, null)

    const restarted = client.request('core.ping')
    const second = spawnProcess.mock.results[1]?.value as FakeCoreProcess
    first.stdout.write('{"jsonrpc":"2.0","method":"agent.event","params":{"delta":"stale"}}\n')
    second.stdout.write('{"jsonrpc":"2.0","id":3,"result":{"alive":true}}\n')
    await expect(restarted).resolves.toEqual({ alive: true })
    expect(received).toHaveBeenCalledOnce()
    expect(log).toHaveBeenCalledOnce()
    client.stop()
    log.mockRestore()
  })

  it('keeps the exit fence when stdin fails before the overload frame arrives', async () => {
    const log = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const client = new CoreJsonRpcClient({
      appDataRoot: resolve('fixtures', 'early-stdin-failure')
    })
    const response = client.request('core.ping')
    const first = spawnProcess.mock.results[0]?.value as FakeCoreProcess
    const pipeError = new Error('write EPIPE')
    first.stdin.emit('error', pipeError)
    await expect(response).rejects.toBe(pipeError)
    await expect(client.request('core.ping')).rejects.toBe(pipeError)
    first.stdout.write(
      `${JSON.stringify({
        jsonrpc: '2.0',
        id: null,
        error: {
          code: -32002,
          message: 'Outbound queue capacity exceeded; this connection is closing',
          data: { code: 'outbound_overloaded', retryable: true }
        }
      })}\n`
    )
    await expect(client.request('core.ping')).rejects.toMatchObject({ code: -32002 })
    expect(spawnProcess).toHaveBeenCalledOnce()
    first.emit('exit', 1, null)
    const restarted = client.request('core.ping')
    const second = spawnProcess.mock.results[1]?.value as FakeCoreProcess
    second.stdout.write('{"jsonrpc":"2.0","id":2,"result":{"alive":true}}\n')
    await expect(restarted).resolves.toEqual({ alive: true })
    expect(spawnProcess).toHaveBeenCalledTimes(2)
    client.stop()
    log.mockRestore()
  })

  it('allows retry after a spawn error without waiting for an exit from a nonexistent process', async () => {
    const client = new CoreJsonRpcClient({ appDataRoot: resolve('fixtures', 'spawn-failure') })
    const response = client.request('core.ping')
    const first = spawnProcess.mock.results[0]?.value as FakeCoreProcess
    first.pid = undefined
    const spawnError = new Error('spawn ENOENT')
    first.emit('error', spawnError)
    await expect(response).rejects.toBe(spawnError)
    const restarted = client.request('core.ping')
    const second = spawnProcess.mock.results[1]?.value as FakeCoreProcess
    second.stdout.write('{"jsonrpc":"2.0","id":2,"result":{"alive":true}}\n')
    await expect(restarted).resolves.toEqual({ alive: true })
    expect(spawnProcess).toHaveBeenCalledTimes(2)
    client.stop()
  })

  it('does not treat an unrelated null-id JSON-RPC error as an overloaded connection', async () => {
    const client = new CoreJsonRpcClient({ appDataRoot: resolve('fixtures', 'ordinary-rpc-error') })
    const response = client.request('core.ping')
    const child = spawnProcess.mock.results[0]?.value as FakeCoreProcess
    child.stdout.write('{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse"}}\n')
    child.stdout.write('{"jsonrpc":"2.0","id":1,"result":{"alive":true}}\n')
    await expect(response).resolves.toEqual({ alive: true })
    client.stop()
  })

  it('can send the graceful shutdown RPC only to the already-running child', async () => {
    const client = new CoreJsonRpcClient({ appDataRoot: resolve('fixtures', 'graceful-shutdown') })
    client.start()
    client.beginShutdown()

    const response = client.requestDuringShutdown<{ stopped: boolean }>('core.shutdown')
    const child = spawnProcess.mock.results[0]?.value as FakeCoreProcess
    child.stdout.write('{"jsonrpc":"2.0","id":1,"result":{"stopped":true}}\n')

    await expect(response).resolves.toEqual({ stopped: true })
    expect(spawnProcess).toHaveBeenCalledOnce()
    client.stop()
    await expect(client.requestDuringShutdown('core.shutdown')).rejects.toThrow(
      'core-server is not running'
    )
    expect(spawnProcess).toHaveBeenCalledOnce()
  })
})
