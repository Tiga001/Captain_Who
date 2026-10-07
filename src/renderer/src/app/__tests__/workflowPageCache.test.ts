import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { WorkflowResponse } from '@mycopilot/protocol'
import {
  getCachedWorkflowPage,
  invalidateWorkflowPages,
  isCurrentWorkflowPage,
  setWorkflowPageAuthScope,
  readWorkflowPage
} from '../../features/workflows/workflowPageCache'

const response = (): WorkflowResponse => ({ records: [], issues: [], instances: [] })
const deferred = () => {
  let resolve!: (value: WorkflowResponse) => void
  const promise = new Promise<WorkflowResponse>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

beforeEach(invalidateWorkflowPages)
afterEach(() => vi.restoreAllMocks())

describe('organization page snapshots', () => {
  it('shares initial and forced background reads but never shares different organizations', async () => {
    const first = deferred()
    const read = vi.fn().mockReturnValue(first.promise)
    const initial = readWorkflowPage('first', read)
    const focus = readWorkflowPage('first', read, true)
    expect(focus).toBe(initial)
    expect(read).toHaveBeenCalledExactlyOnceWith({ operation: 'getInstance', instanceId: 'first' })
    const second = readWorkflowPage('second', read)
    expect(second).not.toBe(initial)
    expect(read).toHaveBeenCalledTimes(2)
    first.resolve(response())
    await Promise.all([initial, focus, second])
  })

  it('cannot repopulate its cache with a read invalidated by a mutation', async () => {
    const beforeMutation = deferred()
    const afterMutation = response()
    const read = vi
      .fn()
      .mockReturnValueOnce(beforeMutation.promise)
      .mockResolvedValue(afterMutation)
    const opening = readWorkflowPage('organization', read)
    invalidateWorkflowPages()
    beforeMutation.resolve(response())
    expect(await opening).toBe(afterMutation)
    expect(read).toHaveBeenCalledTimes(2)
    expect(getCachedWorkflowPage('organization')).toBe(afterMutation)
    expect(isCurrentWorkflowPage('organization', afterMutation)).toBe(true)
    invalidateWorkflowPages()
    expect(isCurrentWorkflowPage('organization', afterMutation)).toBe(false)
  })

  it('surfaces a failed replacement read without repeatedly retrying the obsolete flight', async () => {
    const previous = deferred()
    const failure = new Error('unavailable')
    const read = vi.fn().mockReturnValueOnce(previous.promise).mockRejectedValue(failure)
    const opening = readWorkflowPage('organization', read)
    invalidateWorkflowPages()
    previous.resolve(response())
    await expect(opening).rejects.toBe(failure)
    expect(read).toHaveBeenCalledTimes(2)
    expect(getCachedWorkflowPage('organization')).toBeNull()
  })

  it('clears session snapshots and rejects an old-account flight without replaying it under a new account', async () => {
    setWorkflowPageAuthScope('signedIn:account-a')
    const read = vi.fn().mockResolvedValue(response())
    await readWorkflowPage('warm', read)
    const old = deferred()
    read.mockReturnValueOnce(old.promise)
    const inFlight = readWorkflowPage('loading', read)
    const rejected = expect(inFlight).rejects.toThrow('account session changed')
    setWorkflowPageAuthScope('signedOut:')
    setWorkflowPageAuthScope('signedIn:account-b')
    expect(getCachedWorkflowPage('warm')).toBeNull()
    old.resolve(response())
    await rejected
    expect(getCachedWorkflowPage('loading')).toBeNull()
    expect(read).toHaveBeenCalledTimes(2)
  })

  it('bounds warm snapshots and expires them without retaining a permanent catalog', async () => {
    const now = vi.spyOn(Date, 'now').mockReturnValue(1_000)
    const read = vi.fn().mockImplementation(async () => response())
    await readWorkflowPage(null, read)
    for (let i = 0; i < 33; i++) await readWorkflowPage(String(i), read)
    expect(getCachedWorkflowPage('0')).toBeNull()
    expect(getCachedWorkflowPage('32')).not.toBeNull()
    now.mockReturnValue(16_001)
    expect(getCachedWorkflowPage(null)).toBeNull()
    expect(getCachedWorkflowPage('32')).toBeNull()
  })
})
