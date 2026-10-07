import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import type { AuthState } from '@mycopilot/host-api'
const host = vi.hoisted(() => ({ request: vi.fn(), auth: vi.fn(), runtime: vi.fn() }))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: { requestWorkflows: host.request, onWorkflowRuntimeChanged: host.runtime },
    auth: { onStateChanged: host.auth }
  }
}))
let requestWorkflows: typeof import('../../features/workflows/workflowClient').requestWorkflows
let cache: typeof import('../../features/workflows/workflowPageCache')
const success = { ok: true, value: { records: [], issues: [], instances: [] } }
beforeEach(async () => {
  vi.resetModules()
  host.request.mockReset().mockResolvedValue(success)
  host.auth.mockReset().mockReturnValue(() => undefined)
  host.runtime.mockReset().mockReturnValue(() => undefined)
  ;({ requestWorkflows } = await import('../../features/workflows/workflowClient'))
  cache = await import('../../features/workflows/workflowPageCache')
})

afterEach(() => vi.unstubAllGlobals())

it('invalidates board snapshots on auth changes even with no organization page mounted', async () => {
  const dispatchEvent = vi.fn()
  vi.stubGlobal('window', { dispatchEvent })
  let notify!: (state: AuthState) => void
  host.auth.mockImplementation((callback) => {
    notify = callback
    return () => undefined
  })
  cache.setWorkflowPageAuthScope('signedIn:account-a')
  await cache.readWorkflowPage('organization', requestWorkflows)
  expect(cache.getCachedWorkflowPage('organization')).not.toBeNull()
  expect(dispatchEvent).not.toHaveBeenCalled()
  notify({ revision: 2, status: 'signedOut', profile: null, remembered: false, error: null })
  expect(cache.getCachedWorkflowPage('organization')).toBeNull()
  await requestWorkflows({ operation: 'list' })
  expect(host.auth).toHaveBeenCalledTimes(1)
})

it('shares only identical in-flight recovery reads and never caches completed snapshots', async () => {
  let resolve!: (value: typeof success) => void
  host.request.mockImplementation(
    () =>
      new Promise((done) => {
        resolve = done
      })
  )
  const input = { operation: 'runtimeSnapshot', instanceId: 'org' } as const
  const first = requestWorkflows(input)
  expect(requestWorkflows(input)).toBe(first)
  expect(host.request).toHaveBeenCalledTimes(1)
  resolve(success)
  await first
  const second = requestWorkflows(input)
  expect(second).not.toBe(first)
  expect(host.request).toHaveBeenCalledTimes(2)
  resolve(success)
  await second
  const list = requestWorkflows({ operation: 'listInstances' })
  expect(requestWorkflows({ operation: 'listInstances' })).toBe(list)
  resolve(success)
  await list
})

it('does not substitute cursor queries for full recovery or retain failed reads', async () => {
  let reject!: (reason: unknown) => void
  const failed = new Promise((_, fail) => {
    reject = fail
  })
  host.request.mockReturnValue(failed)
  const reads = [
    requestWorkflows({ operation: 'runtimeSnapshot', instanceId: 'org' }),
    requestWorkflows({ operation: 'runtimeSnapshot', instanceId: 'org', afterSequence: 0 }),
    requestWorkflows({ operation: 'runtimeSnapshot', instanceId: 'org', afterSequence: 5 }),
    requestWorkflows({ operation: 'runtimeSnapshot', instanceId: 'org', summaryOnly: true })
  ]
  expect(host.request).toHaveBeenCalledTimes(4)
  const settled = Promise.allSettled(reads)
  reject(new Error('temporarily unavailable'))
  expect((await settled).every((result) => result.status === 'rejected')).toBe(true)
  host.request.mockResolvedValue(success)
  await requestWorkflows({ operation: 'runtimeSnapshot', instanceId: 'org' })
  expect(host.request).toHaveBeenCalledTimes(5)
})

it('starts fresh reads after a committed notification without disrupting unrelated organizations', async () => {
  let resolve!: (value: typeof success) => void
  host.request.mockReturnValue(
    new Promise((done) => {
      resolve = done
    })
  )
  const a = { operation: 'runtimeSnapshot', instanceId: 'a' } as const
  const b = { operation: 'runtimeSnapshot', instanceId: 'b' } as const
  const first = requestWorkflows(a),
    other = requestWorkflows(b)
  const list = requestWorkflows({ operation: 'listInstances' })
  host.runtime.mock.calls[0][0]({ instanceId: 'a' })
  const next = requestWorkflows(a)
  const nextList = requestWorkflows({ operation: 'listInstances' })
  expect(next).not.toBe(first)
  expect(nextList).not.toBe(list)
  expect(requestWorkflows(b)).toBe(other)
  expect(host.request).toHaveBeenCalledTimes(5)
  resolve(success)
  await Promise.all([first, other, list, next, nextList])
})

it('rejects old-session recovery results and never shares them with a new account', async () => {
  let resolve!: (value: typeof success) => void
  host.request.mockReturnValue(
    new Promise((done) => {
      resolve = done
    })
  )
  const input = { operation: 'runtimeSnapshot', instanceId: 'org' } as const
  const first = requestWorkflows(input)
  const rejected = expect(first).rejects.toThrow('account session changed')
  host.auth.mock.calls[0][0]({ status: 'signedOut', profile: null })
  const next = requestWorkflows(input)
  expect(next).not.toBe(first)
  resolve(success)
  await rejected
  await expect(next).resolves.toEqual(success.value)
})
