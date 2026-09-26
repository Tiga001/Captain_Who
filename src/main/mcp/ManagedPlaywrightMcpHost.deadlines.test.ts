import { afterEach, describe, expect, it, vi } from 'vitest'
import { type ManagedMcpClient } from './ManagedPlaywrightMcpHost'
import { createManagedPlaywrightHostTestFixture } from './ManagedPlaywrightMcpHost.test-fixtures'

const {
  closeTrackedHosts,
  RISK_CONTEXT,
  PARENT_REQUEST_ID,
  sensitiveContext,
  fakeHost,
  riskLease
} = createManagedPlaywrightHostTestFixture()

afterEach(closeTrackedHosts)

describe('ManagedPlaywrightMcpHost', () => {
  it('retires a timed-out evaluate generation before releasing the next dispatch slot', async () => {
    let completeLateEvaluation!: (value: unknown) => void
    let markEvaluationStarted!: () => void
    const evaluationStarted = new Promise<void>((resolve) => (markEvaluationStarted = resolve))
    const lateEvaluation = new Promise<unknown>((resolve) => {
      completeLateEvaluation = resolve
    })
    const callTool = vi.fn(async (input: { name: string }) => {
      if (input.name === 'browser_evaluate') {
        markEvaluationStarted()
        return await lateEvaluation
      }
      return {
        content: [{ type: 'text', text: 'fresh-generation-snapshot' }],
        isError: false
      }
    })
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({
      callTool,
      createOfficialConnection,
      detachAutomation,
      toolTimeoutMs: 25
    })
    const args = {
      function: '() => new Promise(() => undefined)',
      call_reason: 'Exercise a never-settling repository fixture script.'
    }
    let terminalCount = 0
    const timedOut = host
      .callTool('browser_evaluate', args, {
        authorizationContext: sensitiveContext('browser_evaluate', args)
      })
      .finally(() => {
        terminalCount += 1
      })
    await evaluationStarted
    const queuedSnapshot = host.callTool('browser_snapshot', {
      call_reason: 'Inspect the recovered fixture page.'
    })

    await expect(timedOut).rejects.toMatchObject({
      code: 'browser.risk_outcome_unknown',
      dispatchCertainty: 'possibly_dispatched'
    })
    expect(detachAutomation).toHaveBeenCalledOnce()

    await expect(queuedSnapshot).resolves.toEqual({
      content: [{ type: 'text', text: 'fresh-generation-snapshot' }],
      isError: false
    })
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)

    completeLateEvaluation({
      content: [{ type: 'text', text: 'stale-generation-result' }],
      isError: false
    })
    await new Promise<void>((resolve) => setImmediate(resolve))
    expect(terminalCount).toBe(1)
    await expect(
      host.callTool('browser_snapshot', {
        call_reason: 'Verify the recovered generation remains authoritative.'
      })
    ).resolves.toMatchObject({ isError: false })
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
  })

  it('gives a queued call its full execution budget after the previous call finishes', async () => {
    let releaseFirst!: (result: unknown) => void
    let releaseSecond!: (result: unknown) => void
    let markFirstStarted!: () => void
    let markSecondStarted!: () => void
    const firstStarted = new Promise<void>((resolve) => (markFirstStarted = resolve))
    const secondStarted = new Promise<void>((resolve) => (markSecondStarted = resolve))
    const callTool = vi
      .fn<ManagedMcpClient['callTool']>()
      .mockImplementationOnce(async () => {
        markFirstStarted()
        return await new Promise((resolve) => (releaseFirst = resolve))
      })
      .mockImplementationOnce(async () => {
        markSecondStarted()
        return await new Promise((resolve) => (releaseSecond = resolve))
      })
    const host = fakeHost({ callTool })
    await host.connect()
    vi.useFakeTimers()
    try {
      const first = host.callTool(
        'browser_snapshot',
        { call_reason: 'Read the first page.' },
        { timeoutMs: 1_000 }
      )
      await firstStarted
      const second = host.callTool(
        'browser_snapshot',
        { call_reason: 'Read after the current operation.' },
        { timeoutMs: 30 }
      )
      await vi.advanceTimersByTimeAsync(100)
      expect(callTool).toHaveBeenCalledOnce()
      releaseFirst({ content: [], isError: false })
      await first
      await secondStarted
      await vi.advanceTimersByTimeAsync(29)
      releaseSecond({ content: [], isError: false })
      await expect(second).resolves.toMatchObject({ isError: false })
      expect(callTool).toHaveBeenCalledTimes(2)
    } finally {
      vi.useRealTimers()
    }
  })

  it.each(['timeout', 'cancel'] as const)(
    'removes a queued call on %s without dispatching it or retiring the active connection',
    async (stop) => {
      let releaseFirst!: (result: unknown) => void
      let markFirstStarted!: () => void
      const firstStarted = new Promise<void>((resolve) => (markFirstStarted = resolve))
      const callTool = vi
        .fn<ManagedMcpClient['callTool']>()
        .mockImplementationOnce(async () => {
          markFirstStarted()
          return await new Promise((resolve) => (releaseFirst = resolve))
        })
        .mockResolvedValue({ content: [], isError: false })
      const detachAutomation = vi.fn(async () => undefined)
      const host = fakeHost({ callTool, detachAutomation, queueTimeoutMs: 40 })
      await host.connect()
      vi.useFakeTimers()
      try {
        const first = host.callTool('browser_snapshot', { call_reason: 'Keep the slot occupied.' })
        await firstStarted
        const controller = new AbortController()
        const queued = host.callTool(
          'browser_snapshot',
          { call_reason: 'Cancel this queued action.' },
          { signal: controller.signal }
        )
        const rejected = expect(queued).rejects.toMatchObject({
          code:
            stop === 'timeout'
              ? 'mcp.builtin_playwright.queue_timeout'
              : 'mcp.builtin_playwright.cancelled',
          dispatchCertainty: 'definitely_not_dispatched'
        })
        if (stop === 'timeout') await vi.advanceTimersByTimeAsync(41)
        else controller.abort()
        await rejected
        expect(callTool).toHaveBeenCalledOnce()
        expect(detachAutomation).not.toHaveBeenCalled()

        const next = host.callTool('browser_snapshot', {
          call_reason: 'Wait behind the same owner.'
        })
        await vi.advanceTimersByTimeAsync(1)
        expect(callTool).toHaveBeenCalledOnce()
        releaseFirst({ content: [], isError: false })
        await expect(first).resolves.toMatchObject({ isError: false })
        await expect(next).resolves.toMatchObject({ isError: false })
        expect(callTool).toHaveBeenCalledTimes(2)
        expect(detachAutomation).not.toHaveBeenCalled()
      } finally {
        vi.useRealTimers()
      }
    }
  )

  it('pauses the tool deadline only while BrowserRisk waits for a human decision', async () => {
    vi.useFakeTimers()
    try {
      let approveFirst!: () => void
      let approveSecond!: () => void
      let markApprovalStarted!: () => void
      const approvalStarted = new Promise<void>((resolve) => {
        markApprovalStarted = resolve
      })
      let markFirstApprovalSettled!: () => void
      const firstApprovalSettled = new Promise<void>((resolve) => {
        markFirstApprovalSettled = resolve
      })
      let markUpstreamStarted!: () => void
      const upstreamStarted = new Promise<void>((resolve) => {
        markUpstreamStarted = resolve
      })
      const callTool = vi.fn(
        async (
          _input: unknown,
          _schema: undefined,
          options?: { signal?: AbortSignal }
        ): Promise<unknown> => {
          markUpstreamStarted()
          return await new Promise((_resolve, reject) => {
            options?.signal?.addEventListener('abort', () => reject(new Error('aborted')), {
              once: true
            })
          })
        }
      )
      const beginNetworkOperation = vi.fn(async (input) => {
        const risk = riskLease({
          check: async () => {
            input.onApprovalWaitChange?.(true)
            input.onApprovalWaitChange?.(true)
            markApprovalStarted()
            try {
              await new Promise<void>((resolve) => {
                approveFirst = resolve
              })
              input.onApprovalWaitChange?.(false)
              markFirstApprovalSettled()
              await new Promise<void>((resolve) => {
                approveSecond = resolve
              })
            } finally {
              input.onApprovalWaitChange?.(false)
            }
          }
        })
        return risk.lease
      })
      const host = fakeHost({ beginNetworkOperation, callTool, toolTimeoutMs: 50 })

      const pending = host.callTool(
        'browser_navigate',
        { url: 'http://127.0.0.1:3000/', call_reason: 'Open the fixture.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
      await approvalStarted

      await vi.advanceTimersByTimeAsync(5_000)
      expect(callTool).not.toHaveBeenCalled()

      approveFirst()
      await firstApprovalSettled
      await vi.advanceTimersByTimeAsync(5_000)
      expect(callTool).not.toHaveBeenCalled()

      approveSecond()
      await upstreamStarted
      await vi.advanceTimersByTimeAsync(51)

      // The resumed timeout fires after upstream dispatch, so the Host must conservatively expose
      // an outcome-unknown BrowserRisk error rather than claiming a definite timeout result.
      await expect(pending).rejects.toMatchObject({ code: 'browser.risk_outcome_unknown' })
      expect(callTool).toHaveBeenCalledOnce()
    } finally {
      vi.useRealTimers()
    }
  })

  it('still honors caller cancellation while BrowserRisk waits for a human decision', async () => {
    let markApprovalStarted!: () => void
    const approvalStarted = new Promise<void>((resolve) => {
      markApprovalStarted = resolve
    })
    const beginNetworkOperation = vi.fn(async (input) => {
      const risk = riskLease({
        check: async () => {
          input.onApprovalWaitChange?.(true)
          markApprovalStarted()
          try {
            await new Promise<void>((_resolve, reject) => {
              input.signal.addEventListener('abort', () => reject(new Error('cancelled')), {
                once: true
              })
            })
          } finally {
            input.onApprovalWaitChange?.(false)
          }
        }
      })
      return risk.lease
    })
    const callTool = vi.fn(async () => ({ content: [], isError: false }))
    const host = fakeHost({ beginNetworkOperation, callTool, toolTimeoutMs: 50 })
    const controller = new AbortController()

    const pending = host.callTool(
      'browser_navigate',
      { url: 'http://127.0.0.1:3000/', call_reason: 'Open the fixture.' },
      {
        authorizationContext: RISK_CONTEXT,
        parentRequestId: PARENT_REQUEST_ID,
        signal: controller.signal
      }
    )
    await approvalStarted
    controller.abort('task_cancelled')

    await expect(pending).rejects.toMatchObject({ code: 'mcp.builtin_playwright.cancelled' })
    expect(callTool).not.toHaveBeenCalled()
  })
})
