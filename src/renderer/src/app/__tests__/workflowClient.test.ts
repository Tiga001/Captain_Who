import { afterEach, expect, it, vi } from 'vitest'
import type { AuthState } from '@mycopilot/host-api'
const host = vi.hoisted(() => ({ request: vi.fn(), auth: vi.fn() }))
vi.mock('../../host/hostClient', () => ({
  hostClient: { agent: { requestWorkflows: host.request }, auth: { onStateChanged: host.auth } }
}))
import { requestWorkflows } from '../../features/workflows/workflowClient'
import {
  getCachedWorkflowPage,
  readWorkflowPage,
  setWorkflowPageAuthScope
} from '../../features/workflows/workflowPageCache'

afterEach(() => vi.unstubAllGlobals())

it('invalidates board snapshots on auth changes even with no organization page mounted', async () => {
  const dispatchEvent = vi.fn()
  vi.stubGlobal('window', { dispatchEvent })
  let notify!: (state: AuthState) => void
  host.auth.mockImplementation((callback) => {
    notify = callback
    return () => undefined
  })
  host.request.mockResolvedValue({ ok: true, value: { records: [], issues: [], instances: [] } })
  setWorkflowPageAuthScope('signedIn:account-a')
  await readWorkflowPage('organization', requestWorkflows)
  expect(getCachedWorkflowPage('organization')).not.toBeNull()
  expect(dispatchEvent).not.toHaveBeenCalled()
  notify({ revision: 2, status: 'signedOut', profile: null, remembered: false, error: null })
  expect(getCachedWorkflowPage('organization')).toBeNull()
  await requestWorkflows({ operation: 'list' })
  expect(host.auth).toHaveBeenCalledTimes(1)
})
