import { describe, expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { AutomationDrawer } from '../components/AutomationDrawer'
import {
  makeAutomationDraft,
  makeAutomationRun,
  makeAutomationTask,
  testConversation,
  testModel
} from './automationUiFixtures'
import { singleFolderProject } from '../../projects/__tests__/projectFixtures'
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
  maximized: false,
  mode: 'task' as const,
  models: [testModel],
  onAcknowledgeAttention: vi.fn(),
  onClose: vi.fn(),
  onDirtyChange: vi.fn(),
  onHistoryLoadMore: vi.fn(),
  onHistoryRetry: vi.fn(),
  onOpenConversation: vi.fn(),
  onOpenPermissionSettings: vi.fn(),
  onRetryDetail: vi.fn(),
  onSubmittingChange: vi.fn(),
  onSubmit: vi.fn(async () => undefined),
  onToggleMaximized: vi.fn(),
  permissionModeAvailability: { custom: true, full: true },
  projects: [singleFolderProject({ id: 'project-1', name: 'Project One' })],
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
  it('uses the right-sidebar maximize and restore control semantics', async () => {
    const onToggleMaximized = vi.fn()
    const screen = await render(
      <AutomationDrawer
        {...commonProps}
        maximized={false}
        onToggleMaximized={onToggleMaximized}
        task={existingTask}
      />
    )

    const maximize = screen.getByRole('button', { name: 'automation.maximizeDrawer' })
    await expect.element(maximize).toHaveAttribute('aria-pressed', 'false')
    await maximize.click()
    expect(onToggleMaximized).toHaveBeenCalledOnce()

    await screen.rerender(
      <AutomationDrawer
        {...commonProps}
        maximized
        onToggleMaximized={onToggleMaximized}
        task={existingTask}
      />
    )
    await expect
      .element(screen.getByRole('button', { name: 'automation.restoreDrawer' }))
      .toHaveAttribute('aria-pressed', 'true')
  })

  it('keeps the edit header focused on editing and provides static collapse plus maximize controls', async () => {
    const onClose = vi.fn()
    const screen = await render(
      <AutomationDrawer {...commonProps} onClose={onClose} task={existingTask} />
    )

    const heading = screen.getByRole('heading', { name: 'Daily brief' })
    await expect.element(heading).toBeVisible()
    await expect.poll(() => document.activeElement).toBe(heading.element())

    await expect
      .element(screen.getByRole('button', { name: 'automation.moreActions' }))
      .not.toBeInTheDocument()
    await expect
      .element(screen.getByRole('button', { name: 'automation.runNow' }))
      .not.toBeInTheDocument()
    await expect.element(screen.getByText('automation.statusActive')).not.toBeInTheDocument()

    const collapse = screen.getByRole('button', { name: 'automation.collapseDrawer' })
    await expect.element(collapse).toBeVisible()
    expect(collapse.element().querySelector('.panel-toggle__icon')).not.toBeNull()
    await collapse.click()
    expect(onClose).toHaveBeenCalledOnce()
  })

  it('keeps only the precise run-history chat action when a run has a conversation', async () => {
    const onOpenConversation = vi.fn()
    const screen = await render(
      <AutomationDrawer
        {...commonProps}
        onOpenConversation={onOpenConversation}
        runs={[makeAutomationRun()]}
        task={existingTask}
      />
    )

    const openChat = screen.getByRole('button', { name: 'automation.openChat' })
    expect(openChat.elements()).toHaveLength(1)
    await openChat.click()
    expect(onOpenConversation).toHaveBeenCalledWith('conversation-1', 'message-assistant-1')
  })

  it('waits for run history before deciding whether the generic chat fallback is needed', async () => {
    const screen = await render(
      <AutomationDrawer {...commonProps} historyLoading task={existingTask} />
    )

    await expect
      .element(screen.getByRole('button', { name: 'automation.openChat' }))
      .not.toBeInTheDocument()
    await expect.element(screen.getByText('automation.loading')).toBeVisible()
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

    await expect
      .element(screen.getByRole('button', { name: 'automation.cancel', exact: true }))
      .toBeDisabled()
    await expect
      .element(screen.getByRole('button', { name: 'automation.maximizeDrawer' }))
      .toBeEnabled()
    await expect
      .element(screen.getByRole('button', { name: 'automation.collapseDrawer' }))
      .toBeDisabled()
    await userEvent.keyboard('{Escape}')
    expect(onClose).not.toHaveBeenCalled()
  })
})
