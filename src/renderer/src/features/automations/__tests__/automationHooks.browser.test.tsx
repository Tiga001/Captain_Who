import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  AutomationAttention,
  AutomationAttentionAcknowledgeOutput,
  AutomationAttentionSummaryOutput,
  AutomationListOutput,
  AutomationRun,
  AutomationRunsListOutput,
  AutomationTask
} from '@mycopilot/protocol'
import type { UseAutomationAttentionResult } from '../useAutomationAttention'
import type { UseAutomationDetailResult } from '../useAutomationDetail'
import type { UseAutomationRunsResult } from '../useAutomationRuns'
import type { UseAutomationsResult } from '../useAutomations'
import { makeAutomationDraft, makeAutomationRun, makeAutomationTask } from './automationUiFixtures'

interface Deferred<Value> {
  promise: Promise<Value>
  resolve(value: Value): void
  reject(reason: unknown): void
}

function deferred<Value>(): Deferred<Value> {
  let resolve!: (value: Value) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<Value>((next, fail) => {
    resolve = next
    reject = fail
  })
  return { promise, reject, resolve }
}

const client = vi.hoisted(() => ({
  acknowledgeAutomationAttention: vi.fn(),
  createAutomation: vi.fn(),
  deleteAutomation: vi.fn(),
  getAutomation: vi.fn(),
  getAutomationAttention: vi.fn(),
  listAutomationRuns: vi.fn(),
  listAutomations: vi.fn(),
  runAutomationNow: vi.fn(),
  setAutomationEnabled: vi.fn(),
  updateAutomation: vi.fn()
}))

vi.mock('../automationClient', () => {
  class AutomationClientError extends Error {
    constructor(readonly details: Record<string, unknown>) {
      super(String(details.message ?? 'Automation request failed'))
      Object.assign(this, details)
    }
  }
  return {
    ...client,
    AutomationClientError,
    createAutomationRequestId: () => 'request-hook-test',
    getAutomationErrorDetails: (error: unknown) =>
      error && typeof error === 'object' && 'code' in error
        ? error
        : {
            code: 'transport',
            message: error instanceof Error ? error.message : String(error),
            automationId: null,
            currentRevision: null,
            field: null,
            retryable: true
          },
    hasAutomationHostApi: () => true
  }
})

vi.mock('../automationRealtime', () => ({
  getAutomationSequence: () => 0,
  subscribeAutomationRealtime: () => () => undefined,
  synchronizeAutomationSequence: vi.fn()
}))

const { cacheAutomationTask, resetAutomationCacheForTests } = await import('../automationCache')
const { useAutomations } = await import('../useAutomations')
const { useAutomationDetail } = await import('../useAutomationDetail')
const { useAutomationRuns } = await import('../useAutomationRuns')
const { useAutomationAttention } = await import('../useAutomationAttention')

function listOutput(
  tasks: AutomationTask[],
  patch: Partial<AutomationListOutput> = {}
): AutomationListOutput {
  return {
    schemaVersion: 1,
    tasks,
    nextCursor: null,
    counts: {
      all: tasks.length,
      active: tasks.filter((task) => task.status === 'active').length,
      paused: tasks.filter((task) => task.status === 'paused').length
    },
    attentionCount: 0,
    lastSequence: 1,
    ...patch
  }
}

function attention(id: string, readAt: number | null = null): AutomationAttention {
  return {
    schemaVersion: 1,
    attentionId: id,
    automationId: 'automation-1',
    runId: 'run-1',
    kind: 'important_update',
    message: 'Review this update.',
    createdAt: 10,
    readAt
  }
}

describe('automation hook response ordering', () => {
  beforeEach(() => {
    resetAutomationCacheForTests()
    for (const mock of Object.values(client)) mock.mockReset()
  })

  it('uses the cached newer revision when an older list response arrives later', async () => {
    const oldTask = makeAutomationTask({ revision: 2, title: 'old' })
    const newTask = makeAutomationTask({ revision: 3, title: 'new', updatedAt: 30 })
    cacheAutomationTask(newTask)
    client.listAutomations.mockResolvedValue(listOutput([oldTask]))

    let result!: UseAutomationsResult
    const screen = await render(<AutomationsHarness onRender={(value) => (result = value)} />)

    await expect.poll(() => result.status).toBe('ready')
    expect(result.tasks).toHaveLength(1)
    expect(result.tasks[0]).toMatchObject({ revision: 3, title: 'new' })
    await screen.unmount()
  })

  it('does not let an old mutation response replace a newer cached revision', async () => {
    const initial = makeAutomationTask({ revision: 3, title: 'initial', updatedAt: 30 })
    const external = makeAutomationTask({ revision: 4, title: 'external', updatedAt: 40 })
    const oldMutation = deferred<AutomationTask>()
    client.listAutomations.mockResolvedValue(listOutput([initial], { lastSequence: 4 }))
    client.updateAutomation.mockReturnValue(oldMutation.promise)

    let result!: UseAutomationsResult
    const screen = await render(<AutomationsHarness onRender={(value) => (result = value)} />)
    await expect.poll(() => result.status).toBe('ready')

    const mutation = result.update(result.tasks[0]!, makeAutomationDraft())
    cacheAutomationTask(external)
    oldMutation.resolve({ ...initial, title: 'stale mutation response' })
    await mutation

    await expect.poll(() => result.tasks[0]?.revision).toBe(4)
    expect(result.tasks[0]?.title).toBe('external')
    await screen.unmount()
  })

  it('does not alias two different mutations while the first one is in flight', async () => {
    const initial = makeAutomationTask({ revision: 1, status: 'active' })
    const mutation = deferred<AutomationTask>()
    client.listAutomations.mockResolvedValue(listOutput([initial]))
    client.setAutomationEnabled.mockReturnValue(mutation.promise)

    let result!: UseAutomationsResult
    const screen = await render(<AutomationsHarness onRender={(value) => (result = value)} />)
    await expect.poll(() => result.status).toBe('ready')

    const first = result.setEnabled(result.tasks[0]!, false)
    const duplicate = result.setEnabled(result.tasks[0]!, false)
    expect(duplicate).toBe(first)
    expect(client.setAutomationEnabled).toHaveBeenCalledOnce()
    await expect(result.setEnabled(result.tasks[0]!, true)).rejects.toMatchObject({
      retryable: true
    })

    mutation.resolve(makeAutomationTask({ revision: 2, status: 'paused', updatedAt: 20 }))
    await Promise.all([first, duplicate])
    await screen.unmount()
  })

  it('reuses one create request id for duplicate submits without aliasing a different draft', async () => {
    const creation = deferred<AutomationTask>()
    client.listAutomations.mockResolvedValue(listOutput([]))
    client.createAutomation.mockReturnValue(creation.promise)

    let result!: UseAutomationsResult
    const screen = await render(<AutomationsHarness onRender={(value) => (result = value)} />)
    await expect.poll(() => result.status).toBe('ready')

    const draft = makeAutomationDraft()
    const first = result.create(draft)
    const duplicate = result.create({ ...draft })
    expect(duplicate).toBe(first)
    expect(client.createAutomation).toHaveBeenCalledOnce()
    expect(client.createAutomation).toHaveBeenCalledWith(draft, 'request-hook-test')

    await expect(
      result.create({ ...draft, title: 'A genuinely different task' })
    ).rejects.toMatchObject({ retryable: true })
    creation.resolve(makeAutomationTask())
    await Promise.all([first, duplicate])
    await screen.unmount()
  })

  it('keeps a newer detail cache entry when a stale get response completes', async () => {
    const oldTask = makeAutomationTask({ revision: 1, title: 'old detail' })
    const newTask = makeAutomationTask({ revision: 5, title: 'new detail', updatedAt: 50 })
    const response = deferred<AutomationTask>()
    cacheAutomationTask(newTask)
    client.getAutomation.mockReturnValue(response.promise)

    let result!: UseAutomationDetailResult
    const screen = await render(
      <DetailHarness onRender={(value) => (result = value)} automationId="automation-1" />
    )
    response.resolve(oldTask)

    await expect.poll(() => result.isRefreshing).toBe(false)
    expect(result.task).toMatchObject({ revision: 5, title: 'new detail' })
    await screen.unmount()
  })

  it('does not let a late detail error erase a task cached after the request began', async () => {
    const response = deferred<AutomationTask>()
    const newTask = makeAutomationTask({ revision: 5, title: 'new detail', updatedAt: 50 })
    client.getAutomation.mockReturnValue(response.promise)

    let result!: UseAutomationDetailResult
    const screen = await render(
      <DetailHarness onRender={(value) => (result = value)} automationId="automation-1" />
    )
    await expect.poll(() => client.getAutomation.mock.calls.length).toBe(1)
    cacheAutomationTask(newTask)
    response.reject({ code: 'not_found', message: 'old snapshot did not find it' })

    await expect.poll(() => result.task).toMatchObject({ revision: 5, title: 'new detail' })
    expect(result.isRefreshing).toBe(false)
    expect(result.error).toBeNull()
    await screen.unmount()
  })

  it('ignores a late task page after a newer first-page refresh', async () => {
    const initial = makeAutomationTask({ revision: 1, title: 'initial', updatedAt: 10 })
    const stalePage = deferred<AutomationListOutput>()
    const freshRefresh = deferred<AutomationListOutput>()
    client.listAutomations
      .mockResolvedValueOnce(listOutput([initial], { nextCursor: 'cursor-1', lastSequence: 1 }))
      .mockReturnValueOnce(stalePage.promise)
      .mockReturnValueOnce(freshRefresh.promise)

    let result!: UseAutomationsResult
    const screen = await render(<AutomationsHarness onRender={(value) => (result = value)} />)
    await expect.poll(() => result.status).toBe('ready')

    const loadMore = result.loadMore()
    const refresh = result.refresh()
    freshRefresh.resolve(
      listOutput([makeAutomationTask({ revision: 3, title: 'fresh', updatedAt: 30 })], {
        lastSequence: 3
      })
    )
    await refresh
    stalePage.resolve(
      listOutput([makeAutomationTask({ revision: 2, title: 'stale page', updatedAt: 20 })], {
        lastSequence: 2
      })
    )
    await loadMore

    await expect.poll(() => result.tasks[0]).toMatchObject({ revision: 3, title: 'fresh' })
    expect(result.lastSequence).toBe(3)
    await screen.unmount()
  })

  it('ignores a late runs page after a newer first-page refresh', async () => {
    const initialRun = makeAutomationRun({ updatedAt: 10, resultPreview: 'initial' })
    const stalePage = deferred<AutomationRunsListOutput>()
    const freshRefresh = deferred<AutomationRunsListOutput>()
    client.listAutomationRuns
      .mockResolvedValueOnce({
        schemaVersion: 1,
        automationId: 'automation-1',
        runs: [initialRun],
        nextCursor: 'cursor-1'
      } satisfies AutomationRunsListOutput)
      .mockReturnValueOnce(stalePage.promise)
      .mockReturnValueOnce(freshRefresh.promise)

    let result!: UseAutomationRunsResult
    const screen = await render(
      <RunsHarness onRender={(value) => (result = value)} automationId="automation-1" />
    )
    await expect.poll(() => result.status).toBe('ready')

    const loadMore = result.loadMore()
    const refresh = result.refresh()
    freshRefresh.resolve({
      schemaVersion: 1,
      automationId: 'automation-1',
      runs: [{ ...initialRun, updatedAt: 30, resultPreview: 'fresh' }],
      nextCursor: null
    })
    await refresh
    stalePage.resolve({
      schemaVersion: 1,
      automationId: 'automation-1',
      runs: [{ ...initialRun, updatedAt: 20, resultPreview: 'stale page' }],
      nextCursor: null
    })
    await loadMore

    await expect.poll(() => result.runs[0]).toMatchObject({ updatedAt: 30, resultPreview: 'fresh' })
    await screen.unmount()
  })

  it('deduplicates concurrent attention acknowledgements and converges on authority', async () => {
    const unread = attention('attention-1')
    const read = attention('attention-1', 20)
    const acknowledgement = deferred<AutomationAttentionAcknowledgeOutput>()
    client.getAutomationAttention
      .mockResolvedValueOnce({
        schemaVersion: 1,
        unreadCount: 1,
        items: [unread],
        nextCursor: null,
        lastSequence: 1
      } satisfies AutomationAttentionSummaryOutput)
      .mockResolvedValue({
        schemaVersion: 1,
        unreadCount: 0,
        items: [],
        nextCursor: null,
        lastSequence: 2
      } satisfies AutomationAttentionSummaryOutput)
    client.acknowledgeAutomationAttention.mockReturnValue(acknowledgement.promise)

    let result!: UseAutomationAttentionResult
    const screen = await render(<AttentionHarness onRender={(value) => (result = value)} />)
    await expect.poll(() => result.status).toBe('ready')

    const first = result.acknowledge('attention-1')
    const second = result.acknowledge('attention-1')
    expect(first).toBe(second)
    expect(client.acknowledgeAutomationAttention).toHaveBeenCalledOnce()
    acknowledgement.resolve({ schemaVersion: 1, attention: read })
    await Promise.all([first, second])

    await expect.poll(() => result.unreadCount).toBe(0)
    await expect.poll(() => result.items).toEqual([])
    await screen.unmount()
  })
})

function AutomationsHarness({ onRender }: { onRender(value: UseAutomationsResult): void }) {
  const value = useAutomations()
  onRender(value)
  return (
    <output>{value.tasks.map((task) => `${task.automationId}:${task.revision}`).join(',')}</output>
  )
}

function DetailHarness({
  automationId,
  onRender
}: {
  automationId: string
  onRender(value: UseAutomationDetailResult): void
}) {
  const value = useAutomationDetail(automationId)
  onRender(value)
  return <output>{value.task?.revision ?? 'none'}</output>
}

function RunsHarness({
  automationId,
  onRender
}: {
  automationId: string
  onRender(value: UseAutomationRunsResult): void
}) {
  const value = useAutomationRuns(automationId)
  onRender(value)
  return <output>{value.runs.map((run: AutomationRun) => run.updatedAt).join(',')}</output>
}

function AttentionHarness({ onRender }: { onRender(value: UseAutomationAttentionResult): void }) {
  const value = useAutomationAttention()
  onRender(value)
  return <output>{value.unreadCount}</output>
}
