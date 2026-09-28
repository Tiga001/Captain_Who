import { afterEach, describe, expect, it, vi } from 'vitest'
import type { StorageRunningConversationSummary } from '@mycopilot/protocol'
import {
  agentEventChangesRunningConversations,
  buildMenuBarRunningConversationsMenu,
  MenuBarRunningConversationsController,
  selectMenuBarRunningConversations
} from '../menuBarRunningConversations'

const labels = {
  appName: 'Captain Who',
  empty: 'No chats are running',
  quit: 'Quit Captain Who',
  running: 'Running',
  untitled: 'Untitled chat'
}
const active = [{ id: 'active', title: 'Active chat', updatedAt: 1 }]

function setup() {
  vi.useFakeTimers()
  const applyMenu = vi.fn()
  const loadRunningConversationSummaries = vi.fn(async () => active)
  const controller = new MenuBarRunningConversationsController({
    applyMenu,
    labels,
    loadRunningConversationSummaries,
    openConversation: vi.fn(),
    quit: vi.fn()
  })
  return { controller, applyMenu, loadRunningConversationSummaries }
}
afterEach(() => vi.useRealTimers())

describe('Menu-bar running conversations', () => {
  it('normalizes and sorts authoritative summaries', () => {
    expect(
      selectMenuBarRunningConversations(
        [
          { id: 'b', title: '  ', updatedAt: 1 },
          { id: 'a', title: '  First\nchat  ', updatedAt: 1 },
          { id: 'c', title: 'Newest', updatedAt: 2 }
        ],
        labels.untitled
      )
    ).toEqual([
      { id: 'c', title: 'Newest', updatedAt: 2 },
      { id: 'a', title: 'First chat', updatedAt: 1 },
      { id: 'b', title: labels.untitled, updatedAt: 1 }
    ])
  })
  it('opens native chat rows and preserves the quit command', () => {
    const open = vi.fn(),
      quit = vi.fn()
    const menu = buildMenuBarRunningConversationsMenu(active, labels, open, quit)
    expect(menu.map((item) => item.label)).toEqual([
      labels.appName,
      undefined,
      labels.running,
      'Active chat',
      undefined,
      labels.quit
    ])
    menu[3].click?.({} as never, {} as never, {} as never)
    menu.at(-1)?.click?.({} as never, {} as never, {} as never)
    expect(open).toHaveBeenCalledWith('active')
    expect(quit).toHaveBeenCalledOnce()
  })
  it('ignores 100 spaced text events and accepts lifecycle changes', async () => {
    const { controller, loadRunningConversationSummaries } = setup()
    controller.start()
    await vi.advanceTimersByTimeAsync(100)
    for (let i = 0; i < 100; i++) {
      if (agentEventChangesRunningConversations({ type: 'message_delta' })) controller.refresh()
      await vi.advanceTimersByTimeAsync(10)
    }
    expect(loadRunningConversationSummaries).toHaveBeenCalledOnce()
    for (const type of ['started', 'state', 'approval_required', 'done', 'error'] as const) {
      expect(agentEventChangesRunningConversations({ type })).toBe(true)
    }
    for (const type of [
      'command_output',
      'tool_input_progress',
      'message_stream_started',
      'tool_result'
    ] as const) {
      expect(agentEventChangesRunningConversations({ type })).toBe(false)
    }
    controller.dispose()
  })
  it('coalesces bursts without moving the deadline on sustained events', async () => {
    const { controller, applyMenu, loadRunningConversationSummaries } = setup()
    for (let i = 0; i < 100; i++) controller.refresh()
    await vi.advanceTimersByTimeAsync(100)
    expect(loadRunningConversationSummaries).toHaveBeenCalledOnce()
    for (let i = 0; i < 100; i++) {
      controller.refresh()
      await vi.advanceTimersByTimeAsync(10)
    }
    expect(loadRunningConversationSummaries).toHaveBeenCalledTimes(11)
    expect(applyMenu).toHaveBeenCalledOnce()
    controller.dispose()
  })
  it('discards an obsolete response and serializes the follow-up read', async () => {
    const { controller, applyMenu, loadRunningConversationSummaries } = setup()
    let resolve!: (rows: StorageRunningConversationSummary[]) => void
    loadRunningConversationSummaries.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      })
    )
    controller.refresh()
    await vi.advanceTimersByTimeAsync(100)
    controller.refresh()
    loadRunningConversationSummaries.mockResolvedValueOnce([])
    resolve(active)
    await vi.advanceTimersByTimeAsync(0)
    expect(applyMenu).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(100)
    expect(loadRunningConversationSummaries).toHaveBeenCalledTimes(2)
    expect(applyMenu.mock.calls[0][0][3].label).toBe(labels.empty)
    controller.dispose()
  })
  it('preserves the menu on failure and recovers without a retry loop', async () => {
    const { controller, applyMenu, loadRunningConversationSummaries } = setup()
    controller.start()
    await vi.advanceTimersByTimeAsync(100)
    loadRunningConversationSummaries.mockRejectedValueOnce(new Error('Core restarting'))
    controller.refresh()
    await vi.advanceTimersByTimeAsync(1_100)
    expect(loadRunningConversationSummaries).toHaveBeenCalledTimes(2)
    expect(applyMenu).toHaveBeenCalledOnce()
    loadRunningConversationSummaries.mockResolvedValueOnce([])
    await vi.advanceTimersByTimeAsync(4_100)
    expect(loadRunningConversationSummaries).toHaveBeenCalledTimes(3)
    expect(applyMenu).toHaveBeenCalledTimes(2)
    controller.dispose()
  })

  it('still publishes during continuous invalidations and slow reads', async () => {
    const { controller, applyMenu, loadRunningConversationSummaries } = setup()
    loadRunningConversationSummaries.mockImplementation(
      () =>
        new Promise((resolve) => {
          setTimeout(() => resolve(active), 80)
        })
    )
    for (let i = 0; i < 50; i++) {
      controller.refresh()
      await vi.advanceTimersByTimeAsync(20)
    }
    expect(applyMenu).toHaveBeenCalledOnce()
    expect(loadRunningConversationSummaries.mock.calls.length).toBeLessThanOrEqual(6)
    controller.dispose()
    await vi.advanceTimersByTimeAsync(100)
  })

  it('preserves a restart invalidation received during a failing read', async () => {
    const { controller, applyMenu, loadRunningConversationSummaries } = setup()
    let reject!: (error: Error) => void
    loadRunningConversationSummaries.mockReturnValueOnce(
      new Promise((_resolve, fail) => {
        reject = fail
      })
    )
    controller.refresh()
    await vi.advanceTimersByTimeAsync(100)
    controller.refresh()
    reject(new Error('old Core disconnected'))
    await vi.advanceTimersByTimeAsync(200)
    expect(loadRunningConversationSummaries).toHaveBeenCalledTimes(2)
    expect(applyMenu).toHaveBeenCalledOnce()
    controller.dispose()
  })
  it('rebuilds for rename but not timestamps that leave row order unchanged', async () => {
    const { controller, applyMenu, loadRunningConversationSummaries } = setup()
    controller.refresh()
    await vi.advanceTimersByTimeAsync(100)
    loadRunningConversationSummaries.mockResolvedValueOnce([{ ...active[0], updatedAt: 2 }])
    controller.refresh()
    await vi.advanceTimersByTimeAsync(100)
    expect(applyMenu).toHaveBeenCalledOnce()
    loadRunningConversationSummaries.mockResolvedValueOnce([{ ...active[0], title: 'Renamed' }])
    controller.refresh()
    await vi.advanceTimersByTimeAsync(100)
    expect(applyMenu).toHaveBeenCalledTimes(2)
    controller.dispose()
  })
  it('cancels timers and ignores late results after disposal', async () => {
    const { controller, applyMenu, loadRunningConversationSummaries } = setup()
    let resolve!: (rows: StorageRunningConversationSummary[]) => void
    loadRunningConversationSummaries.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      })
    )
    controller.start()
    controller.start()
    await vi.advanceTimersByTimeAsync(100)
    controller.dispose()
    resolve(active)
    controller.refresh()
    await vi.advanceTimersByTimeAsync(10_000)
    expect(applyMenu).not.toHaveBeenCalled()
    expect(loadRunningConversationSummaries).toHaveBeenCalledOnce()
    expect(vi.getTimerCount()).toBe(0)
  })
})
