import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AgentUsageDashboardInput, AgentUsageDashboardOutput } from '@mycopilot/protocol'
import {
  usageDashboardCache,
  UsageDashboardInvalidatedError
} from '../../features/agent/usageDashboardCache'

const host = vi.hoisted(() => ({ getUsageDashboard: vi.fn(), clearUsageRecords: vi.fn() }))
vi.mock('../../host/hostClient', () => ({ hostClient: { agent: host } }))
const { getAgentUsageDashboard, clearAgentUsageRecords } =
  await import('../../features/agent/agentClient')
const input: AgentUsageDashboardInput = { windows: [{ from: 10, to: 20 }] }
const output: AgentUsageDashboardOutput = {
  summary: { requestCount: 0, messageCount: 0, unpricedMessageCount: 0, models: [] },
  buckets: [{ requestCount: 0, messageCount: 0, unpricedMessageCount: 0, models: [] }]
}
beforeEach(() => {
  vi.clearAllMocks()
  usageDashboardCache.invalidate()
})

describe('agent usage dashboard client', () => {
  it('forwards one batched request without changing accounting', async () => {
    host.getUsageDashboard.mockResolvedValue(output)
    expect(await getAgentUsageDashboard(input)).toBe(output)
    expect(host.getUsageDashboard).toHaveBeenCalledOnce()
    expect(host.getUsageDashboard).toHaveBeenCalledWith(input)
  })
  it.each(['success', 'failure'])(
    'fences cached and in-flight data when clearing elsewhere ends in %s',
    async (outcome) => {
      let finish!: () => void
      host.clearUsageRecords.mockImplementation(
        () =>
          new Promise((resolve, reject) => {
            finish = () =>
              outcome === 'success'
                ? resolve({ deletedRecords: 3 })
                : reject(new Error('clear failed'))
          })
      )
      let resolveOld!: (value: AgentUsageDashboardOutput) => void
      const old = usageDashboardCache.load(
        input.windows,
        () =>
          new Promise((resolve) => {
            resolveOld = resolve
          })
      )
      const oldRejected = expect(old).rejects.toBeInstanceOf(UsageDashboardInvalidatedError)
      await Promise.resolve()
      const listener = vi.fn()
      const stop = usageDashboardCache.subscribe(listener)
      const clear = clearAgentUsageRecords({})
      const clearResult = clear.catch((error: unknown) => error)
      expect(usageDashboardCache.isClearing).toBe(true)
      resolveOld(output)
      await oldRejected
      finish()
      await clearResult
      expect(usageDashboardCache.isClearing).toBe(false)
      expect(usageDashboardCache.read(input.windows)).toBeUndefined()
      expect(listener).toHaveBeenCalledTimes(2)
      stop()
    }
  )
})
