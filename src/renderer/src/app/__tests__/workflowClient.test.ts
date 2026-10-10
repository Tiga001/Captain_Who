import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import type { AuthState } from '@mycopilot/host-api'
const host = vi.hoisted(() => ({
  request: vi.fn(),
  auth: vi.fn(),
  runtime: vi.fn(),
  import: vi.fn(),
  export: vi.fn(),
  getState: undefined as (() => Promise<AuthState>) | undefined
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: {
      requestWorkflows: host.request,
      onWorkflowRuntimeChanged: host.runtime,
      importWorkflowTemplate: host.import,
      exportWorkflowTemplate: host.export
    },
    auth: {
      onStateChanged: host.auth,
      get getState() {
        return host.getState
      }
    }
  }
}))
let requestWorkflows: typeof import('../../features/workflows/workflowClient').requestWorkflows
let importWorkflowTemplate: typeof import('../../features/workflows/workflowClient').importWorkflowTemplate
let exportWorkflowTemplate: typeof import('../../features/workflows/workflowClient').exportWorkflowTemplate
let cache: typeof import('../../features/workflows/workflowPageCache')
const success = { ok: true, value: { records: [], issues: [], instances: [] } }
beforeEach(async () => {
  vi.resetModules()
  host.request.mockReset().mockResolvedValue(success)
  host.auth.mockReset().mockReturnValue(() => undefined)
  host.getState = undefined
  host.runtime.mockReset().mockReturnValue(() => undefined)
  host.import.mockReset().mockResolvedValue(success)
  host.export.mockReset().mockResolvedValue({ ok: true, value: { saved: true } })
  ;({ requestWorkflows, importWorkflowTemplate, exportWorkflowTemplate } =
    await import('../../features/workflows/workflowClient'))
  cache = await import('../../features/workflows/workflowPageCache')
})

afterEach(() => vi.unstubAllGlobals())

it('invalidates cached pages and notifies listeners only after a completed import', async () => {
  const dispatchEvent = vi.fn()
  vi.stubGlobal('window', { dispatchEvent })
  await cache.readWorkflowPage('organization', requestWorkflows)
  expect(cache.getCachedWorkflowPage('organization')).not.toBeNull()
  host.import.mockResolvedValueOnce({ ok: true, value: null })
  await expect(importWorkflowTemplate()).resolves.toBeNull()
  expect(cache.getCachedWorkflowPage('organization')).not.toBeNull()
  expect(dispatchEvent).not.toHaveBeenCalled()
  await expect(importWorkflowTemplate()).resolves.toEqual(success.value)
  expect(cache.getCachedWorkflowPage('organization')).toBeNull()
  expect(dispatchEvent).toHaveBeenCalledTimes(1)
  expect(dispatchEvent.mock.calls[0][0].type).toBe('captain:workflows-changed')
})

it('keeps exports read-only whether using the native dialog or the raw request', async () => {
  const dispatchEvent = vi.fn()
  vi.stubGlobal('window', { dispatchEvent })
  await cache.readWorkflowPage('organization', requestWorkflows)
  const input = { id: 'template', expectedRevision: 7 }
  await expect(exportWorkflowTemplate(input)).resolves.toEqual({ saved: true })
  expect(host.export).toHaveBeenCalledWith(input)
  host.export.mockResolvedValueOnce({ ok: true, value: { saved: false } })
  await expect(exportWorkflowTemplate(input)).resolves.toEqual({ saved: false })
  await requestWorkflows({ operation: 'exportTemplateMarkdown', ...input })
  expect(cache.getCachedWorkflowPage('organization')).not.toBeNull()
  expect(dispatchEvent).not.toHaveBeenCalled()
})

it('preserves structured transfer errors without invalidating cached pages', async () => {
  const dispatchEvent = vi.fn()
  vi.stubGlobal('window', { dispatchEvent })
  await cache.readWorkflowPage('organization', requestWorkflows)
  const error = {
    message: 'organization_template_invalid_format',
    code: -32602,
    data: { code: 'organization_template_invalid_format' }
  }
  host.import.mockResolvedValueOnce({ ok: false, error })
  await expect(importWorkflowTemplate()).rejects.toMatchObject(error)
  expect(cache.getCachedWorkflowPage('organization')).not.toBeNull()
  expect(dispatchEvent).not.toHaveBeenCalled()
})

it('rejects an import from the previous account without invalidating the new account pages', async () => {
  const dispatchEvent = vi.fn()
  vi.stubGlobal('window', { dispatchEvent })
  host.getState = vi.fn().mockResolvedValue({
    revision: 1,
    status: 'signedIn',
    profile: { userId: 'account-a' }
  })
  let resolve!: (value: typeof success) => void
  host.import.mockReturnValue(
    new Promise((done) => {
      resolve = done
    })
  )
  const pending = importWorkflowTemplate()
  const rejected = expect(pending).rejects.toThrow('account session changed')
  await vi.waitFor(() => expect(host.import).toHaveBeenCalledTimes(1))
  host.auth.mock.calls[0][0]({ revision: 2, status: 'signedOut', profile: null })
  await cache.readWorkflowPage('new-account-organization', requestWorkflows)
  resolve(success)
  await rejected
  expect(cache.getCachedWorkflowPage('new-account-organization')).not.toBeNull()
  expect(dispatchEvent).not.toHaveBeenCalled()
})

it('bootstraps the current account before importing so its first profile refresh is not a switch', async () => {
  const state = { revision: 1, status: 'signedIn', profile: { userId: 'account-a' } } as AuthState
  host.getState = vi.fn().mockResolvedValue(state)
  let resolve!: (value: typeof success) => void
  host.import.mockReturnValue(
    new Promise((done) => {
      resolve = done
    })
  )
  const pending = importWorkflowTemplate()
  await vi.waitFor(() => expect(host.import).toHaveBeenCalledTimes(1))
  host.auth.mock.calls[0][0]({ ...state, revision: 2 })
  resolve(success)
  await expect(pending).resolves.toEqual(success.value)
  expect(host.getState).toHaveBeenCalledTimes(1)
})

it.each([false, true])(
  'orders bootstrap snapshots against account events by revision (snapshotNewer=%s)',
  async (snapshotNewer) => {
    const state = { revision: 1, status: 'signedIn', profile: { userId: 'account-a' } } as AuthState
    let resolveState!: (value: AuthState) => void
    host.getState = vi.fn().mockReturnValue(
      new Promise((done) => {
        resolveState = done
      })
    )
    let resolveImport!: (value: typeof success) => void
    host.import.mockReturnValue(
      new Promise((done) => {
        resolveImport = done
      })
    )
    const pending = importWorkflowTemplate()
    expect(host.import).not.toHaveBeenCalled()
    const newState = { ...state, revision: 2, profile: { ...state.profile!, userId: 'account-b' } }
    host.auth.mock.calls[0][0](snapshotNewer ? state : newState)
    resolveState(snapshotNewer ? newState : state)
    await vi.waitFor(() => expect(host.import).toHaveBeenCalledTimes(1))
    host.auth.mock.calls[0][0]({ ...newState, revision: 3 })
    resolveImport(success)
    await expect(pending).resolves.toEqual(success.value)
  }
)

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
  host.auth.mock.calls[0][0]({ revision: 1, status: 'signedOut', profile: null })
  const next = requestWorkflows(input)
  expect(next).not.toBe(first)
  resolve(success)
  await rejected
  await expect(next).resolves.toEqual(success.value)
})
