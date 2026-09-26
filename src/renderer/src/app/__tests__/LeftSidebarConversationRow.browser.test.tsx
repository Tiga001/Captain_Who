// Renderer sidebar tests: verify conversation action overlays escape the sidebar clipping boundary.

import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { SidebarConversation } from '../shell/sidebar/leftSidebarTypes'
import { ConversationRow } from '../shell/sidebar/LeftSidebarConversationRow'
import '../shell/sidebar/LeftSidebar.css'

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

  it('keeps mark unread in the conversation context menu', async () => {
    const screen = await render(
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
    )

    screen
      .getByText('Conversation')
      .element()
      .dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 40, clientY: 80 }))

    await expect.element(screen.getByRole('menuitem', { name: 'Pin chat' })).toBeVisible()
    await expect.element(screen.getByRole('menuitem', { name: 'Rename chat' })).toBeVisible()
    await expect.element(screen.getByRole('menuitem', { name: 'Archive chat' })).toBeVisible()
    await expect.element(screen.getByRole('menuitem', { name: 'Mark unread' })).toBeVisible()
  })
})

function attentionRow(
  conversation: SidebarConversation,
  activeConversationId: string | null = null
) {
  return (
    <ConversationRow
      activeConversationId={activeConversationId}
      archiveLabel="Archive chat"
      conversation={conversation}
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
      waitingAnswerLabel="Waiting answer"
    />
  )
}
const attentionConversation: SidebarConversation = {
  id: 'chat',
  title: 'Chat',
  createdAt: 1,
  updatedAt: 1,
  projectId: null,
  isPending: false,
  isWaitingForAnswer: true,
  unreadAt: 1
}

it('shows unanswered asynchronous questions alongside unread while approval takes precedence', async () => {
  const screen = await render(attentionRow(attentionConversation))
  await expect.element(screen.getByText('Waiting answer')).toBeVisible()
  await expect.element(screen.getByLabelText('Unread', { exact: true })).toBeVisible()
  await screen.rerender(attentionRow({ ...attentionConversation, isWaitingForApproval: true }))
  await expect.element(screen.getByText('Waiting approval')).toBeVisible()
  await expect.element(screen.getByText('Waiting answer')).not.toBeInTheDocument()
  await screen.rerender(attentionRow(attentionConversation, 'chat'))
  await expect.element(screen.getByText('Waiting answer')).not.toBeInTheDocument()
  await expect.element(screen.getByLabelText('Unread', { exact: true })).not.toBeInTheDocument()
})

it('falls back to the waiting run state when no shared attention projection is provided', async () => {
  const screen = await render(
    attentionRow({
      ...attentionConversation,
      isWaitingForAnswer: undefined,
      isPending: true,
      messages: [
        {
          id: 'reply',
          role: 'assistant',
          content: '',
          createdAt: 1,
          agentRun: {
            runId: 'run',
            status: 'waiting_for_user_input',
            toolDefinitions: [],
            toolCalls: [],
            toolResults: [],
            approvals: [],
            fileChangeProposals: [],
            timeline: [],
            startedAt: 1
          }
        }
      ]
    })
  )
  await expect.element(screen.getByText('Waiting answer')).toBeVisible()
  await expect.element(screen.getByLabelText('Processing')).toBeInTheDocument()
})
