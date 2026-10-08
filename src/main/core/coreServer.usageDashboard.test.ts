import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AgentUsageDashboardInput } from '@mycopilot/protocol'

const rpc = vi.hoisted(() => ({ request: vi.fn() }))
vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpc.request
  }
}))

import { CoreServer } from './coreServer'

const input = {
  windows: [
    { from: 0, to: 99 },
    { from: 100, to: 199 }
  ]
}
const summary = { requestCount: 2, messageCount: 1, unpricedMessageCount: 0, models: [] }
const output = {
  summary,
  buckets: [
    { ...summary, requestCount: 1, messageCount: 0 },
    { ...summary, requestCount: 1 }
  ]
}

describe('Host usage dashboard boundary', () => {
  beforeEach(() => vi.resetAllMocks())

  it('sends the whole dashboard in one read and keeps the legacy summary method available', async () => {
    rpc.request.mockResolvedValueOnce(output).mockResolvedValueOnce(summary)
    const server = new CoreServer()
    await expect(server.getUsageDashboard(input)).resolves.toEqual(output)
    expect(rpc.request).toHaveBeenCalledExactlyOnceWith('agent.getUsageDashboard', input)
    await expect(server.getUsageSummary({ range: 'last7Days' })).resolves.toEqual(summary)
    expect(rpc.request).toHaveBeenLastCalledWith('agent.getUsageSummary', { range: 'last7Days' })
  })

  it('rejects invalid windows before dispatch and rejects an incomplete response', async () => {
    const server = new CoreServer()
    expect(() => server.getUsageDashboard({ windows: [] })).toThrow()
    expect(() =>
      server.getUsageDashboard({ windows: [{ from: 0 }] } as AgentUsageDashboardInput)
    ).toThrow()
    expect(rpc.request).not.toHaveBeenCalled()
    rpc.request.mockResolvedValue({ summary, buckets: [summary] })
    await expect(server.getUsageDashboard(input)).rejects.toThrow('one bucket')
  })
})
