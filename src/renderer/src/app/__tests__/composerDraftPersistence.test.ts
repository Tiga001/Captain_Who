import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ChatComposerDraft } from '../../features/chat/chatTypes'
import { ComposerDraftPersistenceQueue } from '../composerDraftPersistence'

function draft(message: string, updatedAt: number): ChatComposerDraft {
  return {
    message,
    permissionMode: 'default',
    modelId: 'model-1',
    projectId: null,
    attachments: [],
    skills: [],
    queuedMessages: [],
    updatedAt
  }
}

afterEach(() => {
  vi.useRealTimers()
})

describe('ComposerDraftPersistenceQueue', () => {
  it('coalesces rapid text edits into one lightweight save', async () => {
    vi.useFakeTimers()
    const saveDraft = vi.fn().mockResolvedValue(undefined)
    const saveMessage = vi.fn().mockResolvedValue(true)
    const queue = new ComposerDraftPersistenceQueue({ saveDraft, saveMessage }, (error) => {
      throw error
    })

    queue.scheduleMessage('scope-1', draft('a', 1))
    queue.scheduleMessage('scope-1', draft('ab', 2))
    queue.scheduleMessage('scope-1', draft('abc', 3))
    await vi.advanceTimersByTimeAsync(299)
    expect(saveMessage).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(1)
    await queue.flushAll()
    expect(saveMessage).toHaveBeenCalledTimes(1)
    expect(saveMessage).toHaveBeenCalledWith({ scopeId: 'scope-1', message: 'abc', updatedAt: 3 })
    expect(saveDraft).not.toHaveBeenCalled()
  })

  it('seeds a missing durable row once through the full save path', async () => {
    vi.useFakeTimers()
    const saveDraft = vi.fn().mockResolvedValue(undefined)
    const saveMessage = vi.fn().mockResolvedValue(false)
    const queue = new ComposerDraftPersistenceQueue({ saveDraft, saveMessage }, (error) => {
      throw error
    })
    const latest = draft('first durable text', 10)

    queue.scheduleMessage('new-scope', latest)
    await vi.advanceTimersByTimeAsync(300)
    await queue.flushAll()
    expect(saveMessage).toHaveBeenCalledTimes(1)
    expect(saveDraft).toHaveBeenCalledWith('new-scope', latest)
  })

  it('cancels an older pending text save when a full draft change is persisted', async () => {
    vi.useFakeTimers()
    const saveDraft = vi.fn().mockResolvedValue(undefined)
    const saveMessage = vi.fn().mockResolvedValue(true)
    const queue = new ComposerDraftPersistenceQueue({ saveDraft, saveMessage }, (error) => {
      throw error
    })
    queue.scheduleMessage('scope-1', draft('pending', 1))
    const complete = { ...draft('pending', 2), modelId: 'model-2' }

    await queue.persistNow('scope-1', complete)
    await vi.advanceTimersByTimeAsync(300)
    expect(saveDraft).toHaveBeenCalledTimes(1)
    expect(saveDraft).toHaveBeenCalledWith('scope-1', complete)
    expect(saveMessage).not.toHaveBeenCalled()
  })

  it('serializes a lightweight save behind an in-flight full save for the same scope', async () => {
    let finishFullSave: (() => void) | undefined
    const saveDraft = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finishFullSave = resolve
        })
    )
    const saveMessage = vi.fn().mockResolvedValue(true)
    const queue = new ComposerDraftPersistenceQueue({ saveDraft, saveMessage }, (error) => {
      throw error
    })

    const fullSave = queue.persistNow('scope-1', draft('full', 1))
    await vi.waitFor(() => expect(saveDraft).toHaveBeenCalledOnce())
    queue.scheduleMessage('scope-1', draft('later text', 2))
    const messageSave = queue.flushScope('scope-1')
    await Promise.resolve()
    expect(saveMessage).not.toHaveBeenCalled()

    finishFullSave?.()
    await Promise.all([fullSave, messageSave])
    expect(saveMessage).toHaveBeenCalledWith({
      scopeId: 'scope-1',
      message: 'later text',
      updatedAt: 2
    })
  })

  it('serializes a full save behind an in-flight lightweight save for the same scope', async () => {
    let finishMessageSave: ((found: boolean) => void) | undefined
    const saveDraft = vi.fn().mockResolvedValue(undefined)
    const saveMessage = vi.fn(
      () =>
        new Promise<boolean>((resolve) => {
          finishMessageSave = resolve
        })
    )
    const queue = new ComposerDraftPersistenceQueue({ saveDraft, saveMessage }, (error) => {
      throw error
    })

    queue.scheduleMessage('scope-1', draft('text', 1))
    const messageSave = queue.flushScope('scope-1')
    await vi.waitFor(() => expect(saveMessage).toHaveBeenCalledOnce())
    const fullDraft = { ...draft('text', 2), modelId: 'model-2' }
    const fullSave = queue.persistNow('scope-1', fullDraft)
    await Promise.resolve()
    expect(saveDraft).not.toHaveBeenCalled()

    finishMessageSave?.(true)
    await Promise.all([messageSave, fullSave])
    expect(saveDraft).toHaveBeenCalledWith('scope-1', fullDraft)
  })

  it('flushes pending text immediately for navigation and lifecycle boundaries', async () => {
    vi.useFakeTimers()
    const saveDraft = vi.fn().mockResolvedValue(undefined)
    const saveMessage = vi.fn().mockResolvedValue(true)
    const queue = new ComposerDraftPersistenceQueue({ saveDraft, saveMessage }, (error) => {
      throw error
    })
    queue.scheduleMessage('scope-1', draft('leave now', 4))

    await queue.flushScope('scope-1')
    expect(saveMessage).toHaveBeenCalledTimes(1)
    expect(saveMessage).toHaveBeenCalledWith({
      scopeId: 'scope-1',
      message: 'leave now',
      updatedAt: 4
    })
  })

  it('keeps draining edits accepted while the quit flush is already in flight', async () => {
    let resolveFirstSave: ((found: boolean) => void) | undefined
    const saveDraft = vi.fn().mockResolvedValue(undefined)
    const saveMessage = vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise<boolean>((resolve) => {
            resolveFirstSave = resolve
          })
      )
      .mockResolvedValue(true)
    const queue = new ComposerDraftPersistenceQueue({ saveDraft, saveMessage }, (error) => {
      throw error
    })

    queue.scheduleMessage('scope-1', draft('first', 1))
    const quitting = queue.sealAndFlushAll()
    await vi.waitFor(() => expect(saveMessage).toHaveBeenCalledTimes(1))

    queue.scheduleMessage('scope-1', draft('second', 2))
    resolveFirstSave?.(true)
    await quitting

    expect(saveMessage).toHaveBeenCalledTimes(2)
    expect(saveMessage).toHaveBeenLastCalledWith({
      scopeId: 'scope-1',
      message: 'second',
      updatedAt: 2
    })
  })

  it('cancels and fences a deleted scope before storage removes its durable row', async () => {
    vi.useFakeTimers()
    const saveDraft = vi.fn().mockResolvedValue(undefined)
    const saveMessage = vi.fn().mockResolvedValue(true)
    const queue = new ComposerDraftPersistenceQueue({ saveDraft, saveMessage }, (error) => {
      throw error
    })

    queue.scheduleMessage('deleted-scope', draft('pending', 1))
    await queue.discardScope('deleted-scope')
    queue.scheduleMessage('deleted-scope', draft('must stay deleted', 2))
    await vi.advanceTimersByTimeAsync(300)

    expect(saveMessage).not.toHaveBeenCalled()
    expect(saveDraft).not.toHaveBeenCalled()
  })

  it('can resume a reversible deletion fence and persist the latest draft', async () => {
    const saveDraft = vi.fn().mockResolvedValue(undefined)
    const saveMessage = vi.fn().mockResolvedValue(true)
    const queue = new ComposerDraftPersistenceQueue({ saveDraft, saveMessage }, (error) => {
      throw error
    })

    await queue.discardScope('restored-scope')
    queue.scheduleMessage('restored-scope', draft('ignored while fenced', 1))
    queue.resumeScope('restored-scope')
    await queue.persistNow('restored-scope', draft('latest after rollback', 2))

    expect(saveMessage).not.toHaveBeenCalled()
    expect(saveDraft).toHaveBeenCalledWith('restored-scope', draft('latest after rollback', 2))
  })
})
