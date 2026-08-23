import { describe, expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { AutomationDrawer } from '../components/AutomationDrawer'
import {
  makeAutomationDraft,
  makeAutomationTask,
  testConversation,
  testModel
} from './automationUiFixtures'
import '../ScheduledPage.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))

const commonProps = {
  conversations: [testConversation],
  draft: makeAutomationDraft({
    destination: { kind: 'existing_chat', conversationId: 'conversation-1' },
    notificationPolicy: 'important_updates'
  }),
  historyHasMore: false,
  historyLoading: false,
  historyLoadingMore: false,
  mode: 'task' as const,
  models: [testModel],
  onAcknowledgeAttention: vi.fn(),
  onClose: vi.fn(),
  onDelete: vi.fn(),
  onDirtyChange: vi.fn(),
  onHistoryLoadMore: vi.fn(),
  onHistoryRetry: vi.fn(),
  onOpenConversation: vi.fn(),
  onOpenPermissionSettings: vi.fn(),
  onRetryDetail: vi.fn(),
  onRunNow: vi.fn(),
  onSetEnabled: vi.fn(),
  onSubmittingChange: vi.fn(),
  onSubmit: vi.fn(async () => undefined),
  permissionModeAvailability: { custom: true, full: true },
  projects: [{ id: 'project-1', name: 'Project One', createdAt: 1 }],
  runs: []
}

const existingTask = makeAutomationTask({
  destination: { kind: 'existing_chat', conversationId: 'conversation-1' },
  notificationPolicy: 'important_updates',
  targetSnapshot: {
    projectName: 'Project One',
    conversationTitle: 'Automation chat',
    modelDisplayName: 'Test Model'
  }
})

describe('AutomationDrawer', () => {
  it('focuses its heading and closes nested menus before the drawer', async () => {
    const onClose = vi.fn()
    const screen = await render(
      <AutomationDrawer {...commonProps} onClose={onClose} task={existingTask} />
    )

    const heading = screen.getByRole('heading', { name: 'Daily brief' })
    await expect.element(heading).toBeVisible()
    expect(document.activeElement).toBe(heading.element())

    await screen.getByRole('button', { name: 'automation.moreActions' }).click()
    await expect.element(screen.getByRole('menu')).toBeVisible()
    await userEvent.keyboard('{Escape}')
    await expect.element(screen.getByRole('menu')).not.toBeInTheDocument()
    expect(onClose).not.toHaveBeenCalled()

    await userEvent.keyboard('{Escape}')
    expect(onClose).toHaveBeenCalledOnce()
  })

  it('keeps an existing chat openable when an unrelated permission is blocked', async () => {
    const onOpenConversation = vi.fn()
    const screen = await render(
      <AutomationDrawer
        {...commonProps}
        onOpenConversation={onOpenConversation}
        task={makeAutomationTask({
          ...existingTask,
          health: {
            state: 'blocked',
            code: 'permission_disabled',
            message: 'Permission disabled on the server'
          },
          destination: { kind: 'existing_chat', conversationId: 'conversation-1' }
        })}
      />
    )

    await expect.element(screen.getByText('automation.healthPermissionDisabled')).toBeVisible()
    const openChat = screen.getByRole('button', { name: 'automation.openChat' })
    await expect.element(openChat).toBeEnabled()
    await openChat.click()
    expect(onOpenConversation).toHaveBeenCalledWith('conversation-1', null)
  })

  it('closes nested selects and confirmations one layer at a time with Escape', async () => {
    const onClose = vi.fn()
    const screen = await render(
      <AutomationDrawer {...commonProps} onClose={onClose} task={existingTask} />
    )

    await screen.getByRole('button', { name: /automation.permission:/ }).click()
    await expect.element(screen.getByRole('listbox')).toBeVisible()
    await userEvent.keyboard('{Escape}')
    await expect.element(screen.getByRole('listbox')).not.toBeInTheDocument()
    await expect.element(screen.getByRole('complementary')).toBeVisible()
    expect(onClose).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: /automation.permission:/ }).click()
    await screen.getByRole('option', { name: 'automation.permissionFull' }).click()
    await expect.element(screen.getByRole('dialog')).toBeVisible()
    await userEvent.keyboard('{Escape}')
    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
    await expect.element(screen.getByRole('complementary')).toBeVisible()
    expect(onClose).not.toHaveBeenCalled()
  })

  it('blocks close controls and Escape while a save is pending', async () => {
    const onClose = vi.fn()
    const screen = await render(
      <AutomationDrawer {...commonProps} mutationPending onClose={onClose} task={existingTask} />
    )

    await expect.element(screen.getByRole('button', { name: 'automation.close' })).toBeDisabled()
    await userEvent.keyboard('{Escape}')
    expect(onClose).not.toHaveBeenCalled()
  })
})
