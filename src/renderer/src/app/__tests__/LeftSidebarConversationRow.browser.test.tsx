// Renderer sidebar tests: verify conversation action overlays escape the sidebar clipping boundary.

import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ConversationRow } from '../../components/sidebar/LeftSidebarConversationRow'
import '../../components/sidebar/LeftSidebar.css'

describe('ConversationRow', () => {
  it('portals the archive tooltip outside an overflow-clipped sidebar', async () => {
    const screen = await render(
      <div
        data-testid="sidebar-clip"
        style={{ height: '120px', overflow: 'hidden', position: 'fixed', width: '180px' }}
      >
        <ConversationRow
          activeConversationId={null}
          archiveLabel="Archive chat"
          conversation={{
            id: 'conversation-1',
            projectId: null,
            modelId: null,
            title: 'Conversation',
            messages: [],
            createdAt: 1,
            updatedAt: 1
          }}
          justNow="Now"
          language="en-US"
          markUnreadLabel="Mark unread"
          now={1}
          onArchiveConversation={vi.fn()}
          onMarkConversationUnread={vi.fn()}
          onRenameConversation={vi.fn()}
          onSelectConversation={vi.fn()}
          onTogglePinConversation={vi.fn()}
          pinLabel="Pin chat"
          processingLabel="Processing"
          renameLabel="Rename chat"
          unreadLabel="Unread"
          unpinLabel="Unpin chat"
          waitingApprovalLabel="Waiting approval"
        />
      </div>
    )

    ;(screen.getByRole('button', { name: 'Archive chat' }).element() as HTMLButtonElement).focus()

    const tooltip = screen.getByRole('tooltip')
    await expect.element(tooltip).toHaveAttribute('data-positioned', 'true')
    await expect.element(tooltip).toBeVisible()
    expect(tooltip.element().parentElement).toBe(document.body)
    expect(screen.getByTestId('sidebar-clip').element().contains(tooltip.element())).toBe(false)
  })
})
