import { describe, expect, it, vi } from 'vitest'
import type { StorageChatConversationMetaRecord } from '@mycopilot/protocol'
import {
  buildDockRecentConversationsMenu,
  DockRecentConversationsController,
  selectDockRecentConversations
} from '../dockRecentConversations'

const labels = {
  empty: 'No recent chats',
  header: 'Recent Chats',
  more: 'More',
  untitled: 'Untitled chat'
}

function conversation(
  id: string,
  updatedAt: number,
  title = id
): StorageChatConversationMetaRecord {
  return { createdAt: updatedAt, id, title, updatedAt }
}

describe('Dock recent conversations', () => {
  it('shows only active root metadata ordered by most recent update', () => {
    const entries = selectDockRecentConversations(
      [
        conversation('older', 1),
        { ...conversation('archived', 9), archivedAt: 10 },
        conversation('newer', 3),
        conversation('untitled', 2, '   ')
      ],
      labels.untitled
    )

    expect(entries).toEqual([
      { id: 'newer', title: 'newer', updatedAt: 3 },
      { id: 'untitled', title: labels.untitled, updatedAt: 2 },
      { id: 'older', title: 'older', updatedAt: 1 }
    ])
  })

  it('keeps the first three chats in the Dock menu and all chats in More', () => {
    const openConversation = vi.fn()
    const menu = buildDockRecentConversationsMenu(
      [
        { id: 'four', title: 'Four', updatedAt: 4 },
        { id: 'three', title: 'Three', updatedAt: 3 },
        { id: 'two', title: 'Two', updatedAt: 2 },
        { id: 'one', title: 'One', updatedAt: 1 }
      ],
      labels,
      openConversation
    )

    expect(menu.map((item) => item.label)).toEqual([
      labels.header,
      undefined,
      'Four',
      'Three',
      'Two',
      undefined,
      labels.more
    ])
    const more = menu.at(-1)
    const moreItems = more?.submenu as Array<{ label?: string }>
    expect(moreItems.map((item) => item.label)).toEqual(['Four', 'Three', 'Two', 'One'])
    const firstChat = menu[2]
    expect(firstChat.click).toBeTypeOf('function')
    firstChat.click?.({} as never, {} as never, {} as never)
    expect(openConversation).toHaveBeenCalledWith('four')
  })

  it('only replaces the native menu when the chat catalog changes', async () => {
    const applyMenu = vi.fn()
    const loadConversationMetas = vi.fn(async () => [conversation('latest', 1)])
    const controller = new DockRecentConversationsController({
      applyMenu,
      labels,
      loadConversationMetas,
      openConversation: vi.fn()
    })

    await controller.refreshNow()
    await controller.refreshNow()
    expect(applyMenu).toHaveBeenCalledTimes(1)

    loadConversationMetas.mockResolvedValueOnce([conversation('latest', 2)])
    await controller.refreshNow()
    expect(applyMenu).toHaveBeenCalledTimes(2)
  })
})
