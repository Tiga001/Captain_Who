import type { AgentEvent, GitReviewTarget } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { useGitReviewAutoRefresh } from '../useGitReviewAutoRefresh'

const { listeners, refresh } = vi.hoisted(() => ({
  listeners: new Set<(event: AgentEvent) => void>(),
  refresh: vi.fn<() => Promise<void>>()
}))
vi.mock('../gitReviewClient', () => ({
  onGitReviewAgentEvent: (listener: (event: AgentEvent) => void) => {
    listeners.add(listener)
    return () => listeners.delete(listener)
  }
}))

function Harness({
  active = true,
  loading = false,
  projectId = 'project-1',
  target = { kind: 'uncommitted' }
}: {
  active?: boolean
  loading?: boolean
  projectId?: string
  target?: GitReviewTarget
}) {
  useGitReviewAutoRefresh(projectId, active, target, 'folder-1', loading, refresh)
  return null
}

function fileEvent(projectId = 'project-1', conversationId = 'conversation-1'): AgentEvent {
  return {
    type: 'file_change_updated',
    runId: 'run-1',
    fileChange: {
      schemaVersion: 1,
      transactionId: 'transaction-1',
      projectId,
      conversationId,
      filePath: 'proof.md',
      operation: 'create',
      updateStrategy: null,
      status: 'applied',
      baseRevision: null,
      additions: 310,
      deletions: 0,
      lineCount: 310,
      byteCount: 9435,
      mutationCount: 1,
      nextMutationIndex: 1,
      statsFinal: true,
      summary: null,
      createdAt: 1,
      updatedAt: 2
    }
  }
}
function emit(event: AgentEvent): void {
  listeners.forEach((listener) => listener(event))
}
const tick = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 300))

beforeEach(() => {
  listeners.clear()
  refresh.mockReset().mockResolvedValue(undefined)
})

describe('Git review automatic refresh', () => {
  it('coalesces applied writes for the current project and refreshes again on run completion', async () => {
    await render(<Harness />)
    emit(fileEvent('other-project'))
    await tick()
    expect(refresh).not.toHaveBeenCalled()
    emit(fileEvent())
    emit(fileEvent())
    emit(fileEvent())
    await expect.poll(() => refresh.mock.calls.length).toBe(1)
    emit({ type: 'done', runId: 'run-1', success: true })
    await expect.poll(() => refresh.mock.calls.length).toBe(2)
  })

  it('waits for an in-flight snapshot and retains changes arriving during refresh', async () => {
    const screen = await render(<Harness loading />)
    emit(fileEvent())
    await tick()
    expect(refresh).not.toHaveBeenCalled()
    let resolve!: () => void
    refresh.mockImplementationOnce(
      () =>
        new Promise<void>((done) => {
          resolve = done
        })
    )
    await screen.rerender(<Harness />)
    await expect.poll(() => refresh.mock.calls.length).toBe(1)
    emit(fileEvent())
    await tick()
    expect(refresh).toHaveBeenCalledTimes(1)
    resolve()
    await expect.poll(() => refresh.mock.calls.length).toBe(2)
  })

  it('cancels queued refresh on deactivation, project switch and unmount', async () => {
    const screen = await render(<Harness />)
    emit(fileEvent())
    await screen.rerender(<Harness active={false} />)
    await tick()
    expect(listeners.size).toBe(0)
    expect(refresh).not.toHaveBeenCalled()
    await screen.rerender(<Harness />)
    emit(fileEvent())
    await screen.rerender(<Harness projectId="project-2" />)
    await tick()
    expect(refresh).not.toHaveBeenCalled()
    emit(fileEvent('project-2'))
    await screen.unmount()
    await tick()
    expect(listeners.size).toBe(0)
    expect(refresh).not.toHaveBeenCalled()
  })

  it('filters last-turn events by conversation and leaves immutable commit reviews alone', async () => {
    const screen = await render(
      <Harness target={{ kind: 'lastTurn', conversationId: 'conversation-1' }} />
    )
    emit(fileEvent('project-1', 'conversation-2'))
    await tick()
    expect(refresh).not.toHaveBeenCalled()
    emit(fileEvent())
    await expect.poll(() => refresh.mock.calls.length).toBe(1)
    await screen.rerender(<Harness target={{ kind: 'commit', commitSha: 'abc' }} />)
    emit(fileEvent())
    await tick()
    expect(listeners.size).toBe(0)
    expect(refresh).toHaveBeenCalledTimes(1)
  })

  it('refreshes command-written files even when no FileChange event is emitted', async () => {
    await render(<Harness />)
    emit({
      type: 'command_exited',
      runId: 'command-run',
      conversationId: 'conversation-1',
      assistantMessageId: 'message-1',
      projectId: 'project-1',
      callId: 'call-1',
      sessionId: 'session-1',
      status: 'exited',
      exitCode: 0,
      endedAt: 2,
      latestSequence: 1,
      outputTruncated: false
    })
    await expect.poll(() => refresh.mock.calls.length).toBe(1)
  })
})
