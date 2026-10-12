import type {
  AgentProviderTransitionNotification,
  AgentProviderTransitionOperation
} from '@mycopilot/protocol'
import { HostInvocationError } from '@mycopilot/host-api'
import { act, StrictMode, useEffect, useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const service = vi.hoisted(() => ({
  getStatus: vi.fn(),
  onBlocked: vi.fn(),
  onCompleted: vi.fn(),
  onRequestError: vi.fn(),
  onTransition: vi.fn(),
  preflight: vi.fn(),
  start: vi.fn()
}))

let transitionHandler: ((event: AgentProviderTransitionNotification) => void) | undefined

vi.mock('../../features/agent/agentClient', () => ({
  getProviderTransitionStatus: service.getStatus,
  onProviderTransition: service.onTransition,
  preflightProviderTransition: service.preflight,
  startProviderTransition: service.start
}))

const { useProviderTransition } = await import('../../features/agentRun/useProviderTransition')

const running: Extract<AgentProviderTransitionOperation, { status: 'running' }> = {
  schemaVersion: 1,
  operationId: 'provider-transition-1',
  conversationId: 'conversation-1',
  targetModelId: 'model-2',
  coveredThroughMessageId: 'assistant-old',
  status: 'running',
  startedAt: 10
}

const completed: Extract<AgentProviderTransitionOperation, { status: 'completed' }> = {
  ...running,
  status: 'completed',
  modelId: 'model-2',
  summaryId: 'summary-1',
  completedAt: 20,
  conversationUpdatedAt: 21
}

function TransitionHarness({ loadOnMount = false }: { loadOnMount?: boolean }) {
  const [outcome, setOutcome] = useState('idle')
  const [currentModelId, setCurrentModelId] = useState('model-1')
  const transition = useProviderTransition({
    onBlocked: service.onBlocked,
    onOperationCompleted: (operation) => {
      service.onCompleted(operation)
      setCurrentModelId(operation.modelId)
    },
    onRequestError: service.onRequestError
  })
  const operations = transition.store.operations['conversation-1'] ?? []
  const { loadStatus } = transition
  useEffect(() => {
    if (loadOnMount) void loadStatus('conversation-1')
  }, [loadOnMount, loadStatus])

  return (
    <div>
      <output data-testid="outcome">{outcome}</output>
      <output data-testid="current-model">{currentModelId}</output>
      <output data-testid="confirmation">
        {transition.store.confirmations['conversation-1'] ? 'required' : 'none'}
      </output>
      <output data-testid="operations">
        {operations.map((operation) => `${operation.operationId}:${operation.status}`).join(',')}
      </output>
      <button
        onClick={() => {
          void transition.request('conversation-1', 'model-2').then((result) => {
            setOutcome(result.status)
          })
        }}
        type="button"
      >
        request transition
      </button>
      <button
        onClick={() => {
          void transition
            .request('conversation-1', 'model-2', { allowUnchangedModel: true })
            .then((result) => setOutcome(result.status))
        }}
        type="button"
      >
        request queued transition
      </button>
      <button
        onClick={() => {
          void transition.request('conversation-1', 'model-3').then((result) => {
            setOutcome(result.status)
          })
        }}
        type="button"
      >
        request model 3
      </button>
      <button
        onClick={() => {
          void transition.confirm('conversation-1').then((result) => {
            setOutcome(result.status)
          })
        }}
        type="button"
      >
        confirm transition
      </button>
      <button
        onClick={() => {
          void transition.loadStatus('conversation-1')
        }}
        type="button"
      >
        load history
      </button>
      <button onClick={() => transition.clearConversation('conversation-1')} type="button">
        clear history
      </button>
    </div>
  )
}

beforeEach(() => {
  transitionHandler = undefined
  service.getStatus.mockReset().mockResolvedValue({ operations: [] })
  service.onBlocked.mockReset()
  service.onCompleted.mockReset()
  service.onRequestError.mockReset()
  service.onTransition.mockReset().mockImplementation((handler) => {
    transitionHandler = handler
    return () => {
      if (transitionHandler === handler) transitionHandler = undefined
    }
  })
  service.preflight
    .mockReset()
    .mockImplementation(
      async ({
        conversationId,
        targetModelId
      }: {
        conversationId: string
        targetModelId: string
      }) => ({
        conversationId,
        targetModelId,
        decision: 'requires_compaction',
        reason: 'api_provider_changed',
        operationId:
          targetModelId === running.targetModelId
            ? running.operationId
            : `provider-transition-${targetModelId}`,
        transitionToken: `opaque-token-${targetModelId}`
      })
    )
  service.start.mockReset().mockResolvedValue(running)
})

afterEach(() => vi.useRealTimers())

async function startRunningTransition() {
  const screen = await render(<TransitionHarness />)
  await screen.getByRole('button', { name: 'request transition' }).click()
  await expect.element(screen.getByTestId('confirmation')).toHaveTextContent('required')
  await screen.getByRole('button', { name: 'confirm transition' }).click()
  await expect.element(screen.getByTestId('outcome')).toHaveTextContent('running')
  return screen
}

describe('useProviderTransition terminal reconciliation', () => {
  it('keeps known state without a switch error or retries when a passive read is overloaded', async () => {
    service.getStatus.mockRejectedValue(
      new HostInvocationError({
        message: 'RPC request queue is full. Retry shortly.',
        code: -32001
      })
    )
    const screen = await render(<TransitionHarness />)
    await act(async () => transitionHandler?.(completed))

    await screen.getByRole('button', { name: 'load history' }).click()
    vi.useFakeTimers()
    await vi.advanceTimersByTimeAsync(120_000)

    await expect
      .element(screen.getByTestId('operations'))
      .toHaveTextContent(`${completed.operationId}:completed`)
    expect(service.getStatus).toHaveBeenCalledTimes(1)
    expect(service.onRequestError).not.toHaveBeenCalled()
    expect(service.preflight).not.toHaveBeenCalled()
    expect(service.start).not.toHaveBeenCalled()
  })

  it('coalesces pending passive reads and permits another read after settlement', async () => {
    const status = deferred<{ operations: AgentProviderTransitionOperation[] }>()
    service.getStatus.mockReturnValueOnce(status.promise)
    const screen = await render(<TransitionHarness />)

    for (let index = 0; index < 5; index += 1) {
      await screen.getByRole('button', { name: 'load history' }).click()
    }
    expect(service.getStatus).toHaveBeenCalledTimes(1)
    await act(async () => status.resolve({ operations: [completed] }))
    await screen.getByRole('button', { name: 'load history' }).click()

    expect(service.getStatus).toHaveBeenCalledTimes(2)
    expect(service.onRequestError).not.toHaveBeenCalled()
  })

  it('discards a passive snapshot predating a newer conversation notification', async () => {
    const status = deferred<{ operations: AgentProviderTransitionOperation[] }>()
    service.getStatus.mockReturnValueOnce(status.promise)
    const screen = await render(<TransitionHarness />)
    await screen.getByRole('button', { name: 'load history' }).click()

    await act(async () => transitionHandler?.(completed))
    await act(async () =>
      status.resolve({ operations: [{ ...running, operationId: 'older-running-operation' }] })
    )

    await expect
      .element(screen.getByTestId('operations'))
      .toHaveTextContent(`${completed.operationId}:completed`)
    expect(screen.getByTestId('operations').element().textContent).not.toContain('older-running')
    expect(service.getStatus).toHaveBeenCalledTimes(1)
    expect(service.onCompleted).not.toHaveBeenCalled()
  })

  it('does not revive cleared history or release a newer pending read with an old reply', async () => {
    const oldStatus = deferred<{ operations: AgentProviderTransitionOperation[] }>()
    const newStatus = deferred<{ operations: AgentProviderTransitionOperation[] }>()
    service.getStatus.mockReturnValueOnce(oldStatus.promise).mockReturnValueOnce(newStatus.promise)
    const screen = await render(<TransitionHarness />)
    await screen.getByRole('button', { name: 'load history' }).click()
    await screen.getByRole('button', { name: 'clear history' }).click()
    await screen.getByRole('button', { name: 'load history' }).click()
    await act(async () => oldStatus.resolve({ operations: [running] }))

    expect(screen.getByTestId('operations').element().textContent).toBe('')
    await screen.getByRole('button', { name: 'load history' }).click()
    expect(service.getStatus).toHaveBeenCalledTimes(2)
    await act(async () => newStatus.resolve({ operations: [completed] }))
    await expect
      .element(screen.getByTestId('operations'))
      .toHaveTextContent(`${completed.operationId}:completed`)
  })

  it('ignores a late read from the StrictMode cleanup without invalidating the remounted read', async () => {
    const oldStatus = deferred<{ operations: AgentProviderTransitionOperation[] }>()
    const newStatus = deferred<{ operations: AgentProviderTransitionOperation[] }>()
    service.getStatus.mockReturnValueOnce(oldStatus.promise).mockReturnValueOnce(newStatus.promise)
    const screen = await render(
      <StrictMode>
        <TransitionHarness loadOnMount />
      </StrictMode>
    )
    expect(service.getStatus).toHaveBeenCalledTimes(2)
    await act(async () => oldStatus.resolve({ operations: [running] }))
    expect(screen.getByTestId('operations').element().textContent).toBe('')
    await screen.getByRole('button', { name: 'load history' }).click()
    expect(service.getStatus).toHaveBeenCalledTimes(2)
    await act(async () => newStatus.resolve({ operations: [completed] }))
    await expect
      .element(screen.getByTestId('operations'))
      .toHaveTextContent(`${completed.operationId}:completed`)
  })

  it('settles a passive read after unmount without reporting a switch failure', async () => {
    const status = deferred<{ operations: AgentProviderTransitionOperation[] }>()
    service.getStatus.mockReturnValueOnce(status.promise)
    const screen = await render(<TransitionHarness loadOnMount />)
    await screen.unmount()
    await act(async () =>
      status.reject(new HostInvocationError({ message: 'overloaded', code: -32001 }))
    )

    expect(service.getStatus).toHaveBeenCalledTimes(1)
    expect(service.onRequestError).not.toHaveBeenCalled()
    expect(service.onCompleted).not.toHaveBeenCalled()
  })

  it.each(['request transition', 'request queued transition'])(
    'still reports a rejected active preflight for %s',
    async (buttonName) => {
      service.preflight.mockRejectedValue(
        new HostInvocationError({ message: 'overloaded', code: -32001 })
      )
      const screen = await render(<TransitionHarness />)
      await screen.getByRole('button', { name: buttonName }).click()

      await expect.element(screen.getByTestId('outcome')).toHaveTextContent('failed')
      expect(service.preflight).toHaveBeenCalledTimes(1)
      expect(service.preflight).toHaveBeenCalledWith({
        conversationId: 'conversation-1',
        targetModelId: 'model-2',
        ...(buttonName === 'request queued transition' ? { forSend: true } : {})
      })
      expect(service.onRequestError).toHaveBeenCalledTimes(1)
      expect(service.start).not.toHaveBeenCalled()
    }
  )

  it('waits for lightweight Host readiness before allowing a send without a transition', async () => {
    const ready = {
      conversationId: 'conversation-1',
      targetModelId: 'model-2',
      decision: 'ready_for_send',
      reason: 'same_protocol'
    }
    let resolve!: (value: typeof ready) => void
    service.preflight.mockReturnValue(
      new Promise((done) => {
        resolve = done
      })
    )
    const screen = await render(<TransitionHarness />)
    await screen.getByRole('button', { name: 'request queued transition' }).click()
    expect(service.preflight).toHaveBeenCalledWith({
      conversationId: 'conversation-1',
      targetModelId: 'model-2',
      forSend: true
    })
    await expect.element(screen.getByTestId('outcome')).toHaveTextContent('idle')
    expect(service.start).not.toHaveBeenCalled()
    resolve(ready)
    await expect.element(screen.getByTestId('outcome')).toHaveTextContent('ready')
    expect(service.start).not.toHaveBeenCalled()
    expect(service.onCompleted).not.toHaveBeenCalled()
    await expect.element(screen.getByTestId('confirmation')).toHaveTextContent('none')
  })

  it('does not treat send readiness as permission to switch models', async () => {
    service.preflight.mockResolvedValue({
      conversationId: 'conversation-1',
      targetModelId: 'model-2',
      decision: 'ready_for_send',
      reason: 'same_protocol'
    })
    const screen = await render(<TransitionHarness />)
    await screen.getByRole('button', { name: 'request transition' }).click()
    await expect.element(screen.getByTestId('outcome')).toHaveTextContent('failed')
    expect(service.start).not.toHaveBeenCalled()
    expect(service.onRequestError).toHaveBeenCalledTimes(1)
  })

  it.each([
    ['compatible', 'same_protocol', 'ready', 0],
    ['compatible', 'no_incompatible_history', 'running', 1],
    ['requires_compaction', 'provider_protocol_changed', 'confirmation_required', 0]
  ] as const)(
    'skips queued transition mutation only for a Host-compatible unchanged model: %s / %s',
    async (decision, reason, outcome, starts) => {
      service.preflight.mockResolvedValue({
        conversationId: 'conversation-1',
        targetModelId: 'model-2',
        decision,
        reason,
        operationId: running.operationId,
        transitionToken: 'opaque-token-model-2'
      })
      const screen = await render(<TransitionHarness />)
      await screen.getByRole('button', { name: 'request queued transition' }).click()
      await expect.element(screen.getByTestId('outcome')).toHaveTextContent(outcome)
      expect(service.start).toHaveBeenCalledTimes(starts)
      expect(service.onCompleted).not.toHaveBeenCalled()
    }
  )

  it('keeps manual compatible transitions on the existing commit path', async () => {
    service.preflight.mockResolvedValue({
      conversationId: 'conversation-1',
      targetModelId: 'model-2',
      decision: 'compatible',
      reason: 'same_protocol',
      operationId: running.operationId,
      transitionToken: 'opaque-token-model-2'
    })
    const screen = await render(<TransitionHarness />)
    await screen.getByRole('button', { name: 'request transition' }).click()
    await expect.element(screen.getByTestId('outcome')).toHaveTextContent('running')
    expect(service.start).toHaveBeenCalledTimes(1)
  })

  it.each([false, true])(
    'stops exact reconciliation when Host rejects start after a lost reply: %s',
    async (loseFirstReply) => {
      service.start.mockReset()
      if (loseFirstReply) {
        service.start.mockRejectedValueOnce(
          new HostInvocationError({ message: 'core-server connection lost' })
        )
      }
      service.start.mockRejectedValue(
        new HostInvocationError({
          message: 'The model-switch check expired. Please try again.',
          code: -32000
        })
      )
      const screen = await render(<TransitionHarness />)
      await screen.getByRole('button', { name: 'request transition' }).click()
      await expect.element(screen.getByTestId('confirmation')).toHaveTextContent('required')
      vi.useFakeTimers()
      await screen.getByRole('button', { name: 'confirm transition' }).click()
      await expect.element(screen.getByTestId('outcome')).toHaveTextContent('failed')

      const statusCalls = service.getStatus.mock.calls.length
      await vi.advanceTimersByTimeAsync(120_000)
      expect(service.getStatus).toHaveBeenCalledTimes(statusCalls)
      expect(service.start).toHaveBeenCalledTimes(loseFirstReply ? 2 : 1)
      expect(service.onRequestError).toHaveBeenCalledTimes(1)
      expect(service.onCompleted).not.toHaveBeenCalled()
    }
  )

  it('recovers a completed exact operation when its terminal notification is lost', async () => {
    service.getStatus.mockImplementation(async (input: { operationId?: string }) => ({
      operations: input.operationId === running.operationId ? [completed] : []
    }))

    const screen = await startRunningTransition()

    await expect
      .poll(() =>
        service.getStatus.mock.calls.some(
          ([input]) =>
            input.conversationId === running.conversationId &&
            input.operationId === running.operationId
        )
      )
      .toBe(true)
    await expect
      .element(screen.getByTestId('operations'))
      .toHaveTextContent(`${running.operationId}:completed`)
    expect(service.onCompleted).toHaveBeenCalledTimes(1)
    expect(service.onCompleted).toHaveBeenCalledWith(completed)
  })

  it('loads historical completed receipts for presentation without replaying model mutation', async () => {
    service.getStatus.mockResolvedValueOnce({ operations: [completed] })
    const screen = await render(<TransitionHarness />)

    await screen.getByRole('button', { name: 'load history' }).click()

    await expect
      .element(screen.getByTestId('operations'))
      .toHaveTextContent(`${completed.operationId}:completed`)
    expect(service.onCompleted).not.toHaveBeenCalled()
  })

  it('deduplicates a terminal notification racing the exact-operation status response', async () => {
    const status = deferred<{ operations: AgentProviderTransitionOperation[] }>()
    service.getStatus.mockImplementation((input: { operationId?: string }) =>
      input.operationId ? status.promise : Promise.resolve({ operations: [] })
    )
    await startRunningTransition()
    await expect.poll(() => service.getStatus.mock.calls.length).toBe(1)

    transitionHandler?.(completed)
    transitionHandler?.(completed)
    status.resolve({ operations: [completed] })

    await expect.poll(() => service.onCompleted.mock.calls.length).toBe(1)
    expect(service.onCompleted).toHaveBeenCalledWith(completed)
  })

  it('projects an exact failed terminal status without invoking completion', async () => {
    const failed: Extract<AgentProviderTransitionOperation, { status: 'failed' }> = {
      ...running,
      status: 'failed',
      completedAt: 20,
      error: {
        code: 'provider_transition_generation_failed',
        message: 'History compression failed.',
        recovery: 'retry'
      }
    }
    service.getStatus.mockResolvedValue({ operations: [failed] })
    const screen = await startRunningTransition()

    await expect
      .element(screen.getByTestId('operations'))
      .toHaveTextContent(`${running.operationId}:failed`)
    expect(service.onCompleted).not.toHaveBeenCalled()
  })

  it('accepts only the preflight-bound exact terminal received before the start reply', async () => {
    const startReply = deferred<AgentProviderTransitionOperation>()
    service.start.mockReset().mockReturnValue(startReply.promise)
    const screen = await render(<TransitionHarness />)

    await screen.getByRole('button', { name: 'request transition' }).click()
    await expect.element(screen.getByTestId('confirmation')).toHaveTextContent('required')
    await screen.getByRole('button', { name: 'confirm transition' }).click()
    await expect.poll(() => service.start.mock.calls.length).toBe(1)

    transitionHandler?.(completed)
    await expect.element(screen.getByTestId('current-model')).toHaveTextContent('model-2')
    expect(service.onCompleted).toHaveBeenCalledTimes(1)

    startReply.resolve(running)
    await expect.element(screen.getByTestId('outcome')).toHaveTextContent('completed')
    await expect.element(screen.getByTestId('current-model')).toHaveTextContent('model-2')
    expect(service.onCompleted).toHaveBeenCalledTimes(1)
    expect(service.onCompleted).toHaveBeenCalledWith(completed)
  })

  it('uses start causality instead of timestamps or opaque operation-id ordering', async () => {
    const runningA: Extract<AgentProviderTransitionOperation, { status: 'running' }> = {
      ...running,
      operationId: 'provider-transition-z',
      targetModelId: 'model-2',
      startedAt: 100
    }
    const completedA: Extract<AgentProviderTransitionOperation, { status: 'completed' }> = {
      ...runningA,
      status: 'completed',
      modelId: 'model-2',
      summaryId: 'summary-a',
      completedAt: 110,
      conversationUpdatedAt: 111
    }
    const runningB: Extract<AgentProviderTransitionOperation, { status: 'running' }> = {
      ...running,
      operationId: 'provider-transition-a',
      targetModelId: 'model-3',
      startedAt: 100
    }
    const completedB: Extract<AgentProviderTransitionOperation, { status: 'completed' }> = {
      ...runningB,
      status: 'completed',
      modelId: 'model-3',
      summaryId: 'summary-b',
      completedAt: 120,
      conversationUpdatedAt: 121
    }
    service.start.mockReset().mockResolvedValueOnce(runningA).mockResolvedValueOnce(runningB)
    service.preflight.mockImplementation(
      async ({
        conversationId,
        targetModelId
      }: {
        conversationId: string
        targetModelId: string
      }) => ({
        conversationId,
        targetModelId,
        decision: 'requires_compaction',
        reason: 'api_provider_changed',
        operationId:
          targetModelId === runningA.targetModelId ? runningA.operationId : runningB.operationId,
        transitionToken: `opaque-token-${targetModelId}`
      })
    )
    const screen = await render(<TransitionHarness />)

    await screen.getByRole('button', { name: 'request transition' }).click()
    await expect.element(screen.getByTestId('confirmation')).toHaveTextContent('required')
    await screen.getByRole('button', { name: 'confirm transition' }).click()
    await expect.element(screen.getByTestId('outcome')).toHaveTextContent('running')
    transitionHandler?.(completedA)
    await expect.element(screen.getByTestId('current-model')).toHaveTextContent('model-2')

    await screen.getByRole('button', { name: 'request model 3' }).click()
    await expect.element(screen.getByTestId('confirmation')).toHaveTextContent('required')
    await screen.getByRole('button', { name: 'confirm transition' }).click()
    await expect.element(screen.getByTestId('outcome')).toHaveTextContent('running')
    transitionHandler?.(completedB)
    await expect.element(screen.getByTestId('current-model')).toHaveTextContent('model-3')

    // A has the lexically larger opaque ID and the same start timestamp. Its late duplicate must
    // remain presentation-only and cannot overwrite the causally newer B attempt.
    transitionHandler?.(completedA)

    expect(service.onCompleted.mock.calls.map(([operation]) => operation.modelId)).toEqual([
      'model-2',
      'model-3'
    ])
    await expect.element(screen.getByTestId('current-model')).toHaveTextContent('model-3')
  })

  it('reconciles the exact operation when both idempotent start replies are lost', async () => {
    service.start.mockReset().mockRejectedValue(new HostInvocationError({ message: 'reply lost' }))
    let exactStatusCalls = 0
    service.getStatus.mockImplementation(async (input: { operationId?: string }) => {
      if (input.operationId !== running.operationId) return { operations: [] }
      exactStatusCalls += 1
      return { operations: [exactStatusCalls === 1 ? running : completed] }
    })

    const screen = await render(<TransitionHarness />)
    await screen.getByRole('button', { name: 'request transition' }).click()
    await expect.element(screen.getByTestId('confirmation')).toHaveTextContent('required')
    await screen.getByRole('button', { name: 'confirm transition' }).click()

    await expect.element(screen.getByTestId('outcome')).toHaveTextContent('running')
    await expect.poll(() => service.start.mock.calls.length).toBe(2)
    await expect
      .poll(() =>
        service.getStatus.mock.calls.some(
          ([input]) =>
            input.conversationId === running.conversationId &&
            input.operationId === running.operationId
        )
      )
      .toBe(true)
    await expect.element(screen.getByTestId('current-model')).toHaveTextContent('model-2')
    expect(service.onCompleted).toHaveBeenCalledTimes(1)
    expect(service.onCompleted).toHaveBeenCalledWith(completed)
  })

  it('keeps exact reconciliation alive beyond the former bounded polling window', async () => {
    let exactStatusCalls = 0
    service.getStatus.mockImplementation(async (input: { operationId?: string }) => {
      if (input.operationId !== running.operationId) return { operations: [] }
      exactStatusCalls += 1
      return { operations: [exactStatusCalls <= 10 ? running : completed] }
    })
    const screen = await render(<TransitionHarness />)
    await screen.getByRole('button', { name: 'request transition' }).click()
    await expect.element(screen.getByTestId('confirmation')).toHaveTextContent('required')

    vi.useFakeTimers()
    await screen.getByRole('button', { name: 'confirm transition' }).click()
    await vi.advanceTimersByTimeAsync(121_000)

    expect(exactStatusCalls).toBeGreaterThan(10)
    expect(service.onCompleted).toHaveBeenCalledTimes(1)
    expect(service.onCompleted).toHaveBeenCalledWith(completed)
    await expect.element(screen.getByTestId('current-model')).toHaveTextContent('model-2')
  })
})

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}
