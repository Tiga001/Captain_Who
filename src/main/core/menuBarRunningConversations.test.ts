import { describe, expect, it, vi } from 'vitest'
import type { StorageChatConversationRecord } from '@mycopilot/protocol'
import {
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

function conversation(
  id: string,
  updatedAt: number,
  messages: StorageChatConversationRecord['messages'],
  title = id
): StorageChatConversationRecord {
  return { createdAt: updatedAt, id, messages, title, updatedAt }
}

function assistantMessage(status: 'pending' | 'sent', agentRunStatus?: string) {
  return {
    id: `message-${status}-${agentRunStatus ?? 'none'}`,
    role: 'assistant',
    content: '',
    createdAt: 1,
    status,
    ...(agentRunStatus ? { agentRunJson: JSON.stringify({ status: agentRunStatus }) } : {})
  }
}

describe('Menu-bar running conversations', () => {
  it('includes root conversations with a pending reply or active run and excludes settled chats', () => {
    const running = selectMenuBarRunningConversations(
      [
        conversation('pending', 2, [assistantMessage('pending')]),
        conversation('approval', 3, [assistantMessage('sent', 'waiting_for_approval')]),
        conversation('done', 4, [assistantMessage('sent', 'completed')]),
        { ...conversation('archived', 5, [assistantMessage('pending')]), archivedAt: 6 }
      ],
      labels.untitled
    )

    expect(running).toEqual([
      { id: 'approval', title: 'approval', updatedAt: 3 },
      { id: 'pending', title: 'pending', updatedAt: 2 }
    ])
  })

  it('provides native running chat rows and a quit command', () => {
    const openConversation = vi.fn()
    const quit = vi.fn()
    const menu = buildMenuBarRunningConversationsMenu(
      [{ id: 'conversation-1', title: 'Active chat', updatedAt: 1 }],
      labels,
      openConversation,
      quit
    )

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
    expect(openConversation).toHaveBeenCalledWith('conversation-1')
    expect(quit).toHaveBeenCalledOnce()
  })

  it('coalesces refresh requests and rebuilds only when running chats change', async () => {
    const applyMenu = vi.fn()
    const loadConversations = vi.fn(async () => [
      conversation('active', 1, [assistantMessage('pending')])
    ])
    const controller = new MenuBarRunningConversationsController({
      applyMenu,
      labels,
      loadConversations,
      openConversation: vi.fn(),
      quit: vi.fn()
    })

    controller.refresh()
    controller.refresh()
    await vi.waitFor(() => expect(applyMenu).toHaveBeenCalledTimes(1))
    expect(loadConversations).toHaveBeenCalledOnce()

    controller.refresh()
    await vi.waitFor(() => expect(loadConversations).toHaveBeenCalledTimes(2))
    expect(applyMenu).toHaveBeenCalledTimes(1)
  })
})
