import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcStart = vi.hoisted(() => vi.fn())
const rpcStop = vi.hoisted(() => vi.fn())
const rpcBeginShutdown = vi.hoisted(() => vi.fn())
const rpcIsRunning = vi.hoisted(() => vi.fn())
const rpcRequest = vi.hoisted(() => vi.fn())
const rpcRequestDuringShutdown = vi.hoisted(() => vi.fn())
const onNotification = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly start = rpcStart
    readonly stop = rpcStop
    readonly beginShutdown = rpcBeginShutdown
    readonly isRunning = rpcIsRunning
    readonly request = rpcRequest
    readonly requestDuringShutdown = rpcRequestDuringShutdown
    readonly onNotification = onNotification
  }
}))

import { CoreServer } from './coreServer'

describe('CoreServer system notification transport', () => {
  beforeEach(() => {
    rpcStart.mockReset()
    rpcStop.mockReset()
    rpcBeginShutdown.mockReset()
    rpcIsRunning.mockReset().mockReturnValue(false)
    rpcRequest.mockReset()
    rpcRequestDuringShutdown.mockReset()
    onNotification.mockReset().mockReturnValue(() => undefined)
  })

  it('subscribes before Core startup and replays only the generic notification resync', () => {
    const receivers = new Map<string, (params: unknown) => void>()
    onNotification.mockImplementation((method, handler) => {
      receivers.set(method, handler)
      return () => undefined
    })
    const server = new CoreServer()
    server.start()
    receivers.get('notification.resync')?.({
      schemaVersion: 1,
      reason: 'core_started',
      lastSequence: 9,
      occurredAt: 100
    })

    const handler = vi.fn()
    server.onNotificationResync(handler)
    expect(handler).toHaveBeenCalledWith({
      schemaVersion: 1,
      reason: 'core_started',
      lastSequence: 9,
      occurredAt: 100
    })
    expect(receivers.has('automation.resync')).toBe(true)
    expect(receivers.has('notification.resync')).toBe(true)
  })

  it('strictly fences batch claim, validation and acknowledge identities', async () => {
    const server = new CoreServer()
    rpcRequest.mockResolvedValueOnce({
      schemaVersion: 1,
      claimToken: 'wrong-claim',
      batches: []
    })
    await expect(
      server.claimNotificationBatches({
        schemaVersion: 1,
        claimToken: 'claim-1',
        leaseDurationMs: 60_000,
        limit: 10
      })
    ).rejects.toThrow('Invalid notification batch claim response identity')

    rpcRequest.mockResolvedValueOnce({
      schemaVersion: 1,
      batchId: 'batch-other',
      batch: null
    })
    await expect(
      server.validateNotificationBatch({
        schemaVersion: 1,
        batchId: 'batch-1',
        claimToken: 'claim-1'
      })
    ).rejects.toThrow('Invalid notification batch validation response identity')

    rpcRequest.mockResolvedValueOnce({
      schemaVersion: 1,
      batchId: 'batch-1',
      status: 'displayed',
      disposition: 'suppressed_foreground',
      acknowledgedAt: 100
    })
    await expect(
      server.acknowledgeNotificationBatch({
        schemaVersion: 1,
        batchId: 'batch-1',
        claimToken: 'claim-1',
        disposition: 'delivered',
        nativePriority: 'completed',
        soundLevelPlayed: 'initial',
        nativeRevision: 1
      })
    ).rejects.toThrow('Invalid notification batch acknowledge response identity')
  })

  it('rejects a valid settings object that did not advance the CAS revision', async () => {
    rpcRequest.mockResolvedValue({
      schemaVersion: 1,
      settings: {
        schemaVersion: 1,
        enabled: true,
        soundEnabled: true,
        showTaskContent: true,
        humanCompletedEnabled: true,
        humanFailedEnabled: true,
        humanApprovalEnabled: true,
        humanCancelledEnabled: true,
        revision: 2,
        updatedAt: 100
      }
    })
    await expect(
      new CoreServer().updateNotificationSettings({
        schemaVersion: 1,
        expectedRevision: 2,
        settings: {
          enabled: true,
          soundEnabled: true,
          showTaskContent: true,
          humanCompletedEnabled: true,
          humanFailedEnabled: true,
          humanApprovalEnabled: true,
          humanCancelledEnabled: true
        }
      })
    ).rejects.toThrow('Invalid notification settings revision response')
  })
})
