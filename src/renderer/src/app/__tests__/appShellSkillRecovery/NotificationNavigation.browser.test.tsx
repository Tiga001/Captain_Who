import type { NotificationOpenRequest } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { testState } from './state'
import { renderSelectedConversation } from './setup'

describe('native notification navigation', () => {
  it('opens the exact conversation message without an in-app notification surface', async () => {
    const screen = await renderSelectedConversation()
    await expect.poll(() => testState.notificationOpenRequestListeners.size).toBe(1)

    const request: NotificationOpenRequest = {
      schemaVersion: 1,
      batchId: 'batch-conversation',
      eventIds: ['notification-conversation'],
      destination: {
        kind: 'conversation',
        conversationId: 'conversation-a',
        messageId: 'assistant-a',
        approvalActionId: null
      }
    }
    for (const listener of testState.notificationOpenRequestListeners) listener(request)

    expect(testState.markNotificationsSeen).toHaveBeenCalledWith({
      kind: 'events',
      eventIds: ['notification-conversation']
    })
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect
      .element(screen.getByTestId('scroll-target-message-id'))
      .toHaveTextContent('assistant-a')
    expect(screen.container.querySelector('.notification-center')).toBeNull()
  })

  it('opens the selected automation run directly', async () => {
    const screen = await renderSelectedConversation()
    await expect.poll(() => testState.notificationOpenRequestListeners.size).toBe(1)

    const request: NotificationOpenRequest = {
      schemaVersion: 1,
      batchId: 'batch-automation',
      eventIds: ['notification-automation'],
      destination: {
        kind: 'automation',
        automationId: 'automation-notification',
        runId: 'run-notification'
      }
    }
    for (const listener of testState.notificationOpenRequestListeners) listener(request)

    await expect
      .element(screen.getByTestId('scheduled-open-automation'))
      .toHaveTextContent('automation-notification')
    await expect
      .element(screen.getByTestId('scheduled-open-run'))
      .toHaveTextContent('run-notification')
  })

  it('leaves the current workspace unchanged for an application-only fallback', async () => {
    const screen = await renderSelectedConversation()
    const chatSurface = screen.getByTestId('active-conversation-id').element()
    const rightSurface = screen.getByTestId('right-sidebar-conversation-id').element()
    await expect.poll(() => testState.notificationOpenRequestListeners.size).toBe(1)
    await screen.getByRole('button', { name: 'open-settings' }).click()
    await expect.element(screen.getByTestId('settings-view')).toBeInTheDocument()

    const request: NotificationOpenRequest = {
      schemaVersion: 1,
      batchId: 'batch-application',
      eventIds: ['notification-application'],
      destination: { kind: 'application' }
    }
    for (const listener of testState.notificationOpenRequestListeners) listener(request)

    expect(screen.getByTestId('active-conversation-id').element()).toBe(chatSurface)
    expect(screen.getByTestId('right-sidebar-conversation-id').element()).toBe(rightSurface)
    await expect.element(screen.getByTestId('settings-view')).toBeInTheDocument()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
  })
})
