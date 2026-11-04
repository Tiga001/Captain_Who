import { describe, expect, it } from 'vitest'
import { testState } from './state'
import { mockSuccessfulTurnStarts } from './turnFixtures'
import { emitAgentEvent } from './eventFixtures'
import { renderSelectedConversation } from './setup'

describe('scheduled workspace isolation', () => {
  it.each([false, true])(
    'marks a completed selected conversation unread only when covered by another page: %s',
    async (covered) => {
      mockSuccessfulTurnStarts()
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'submit-without-skill' }).click()
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
      if (covered) {
        await screen.getByRole('button', { name: 'open-scheduled' }).click()
        await expect.element(screen.getByTestId('scheduled-page-layer')).toBeVisible()
      }
      emitAgentEvent({
        type: 'done',
        runId: 'run-1',
        success: true,
        status: 'completed',
        content: 'Finished while this page was open.'
      })
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
      await expect
        .element(screen.getByTestId('unread-conversation-a'))
        .toHaveTextContent(String(covered))
      if (covered) {
        await expect
          .poll(() => testState.persistedConversations.get('conversation-a')?.unreadAt)
          .toEqual(expect.any(Number))
        expect(screen.getByTestId('active-conversation-id').element().textContent).toBe(
          'conversation-a'
        )
        await screen.getByRole('button', { name: 'select-conversation-a' }).click()
        await screen.getByRole('button', { name: 'confirm-external-navigation' }).click()
        await expect.element(screen.getByTestId('unread-conversation-a')).toHaveTextContent('false')
      }
    }
  )

  it('keeps the selected conversation, composer draft, and right workspace mounted while covered', async () => {
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')

    const mainPanel = screen.container.querySelector<HTMLElement>('.main-panel')
    const rightPanel = screen.container.querySelector<HTMLElement>('.side-panel--right')
    const chatSurface = screen.getByTestId('active-conversation-id').element()
    const draftSurface = screen.getByTestId('draft-model-id').element()
    const rightSurface = screen.getByTestId('right-sidebar-conversation-id').element()
    expect(mainPanel).not.toBeNull()
    expect(rightPanel).not.toBeNull()
    expect(chatSurface.textContent).toBe('conversation-a')
    expect(rightSurface.textContent).toBe('conversation-a')
    await expect
      .element(screen.getByTestId('sidebar-active-conversation'))
      .toHaveTextContent('conversation-a')

    await screen.getByRole('button', { name: 'open-scheduled' }).click()
    await expect.element(screen.getByTestId('scheduled-page-layer')).toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: 'app.expandLeftSidebar' }))
      .toBeVisible()

    expect(screen.container.querySelector('.main-panel')).toBe(mainPanel)
    expect(screen.container.querySelector('.side-panel--right')).toBe(rightPanel)
    expect(screen.getByTestId('active-conversation-id').element()).toBe(chatSurface)
    expect(screen.getByTestId('draft-model-id').element()).toBe(draftSurface)
    expect(screen.getByTestId('right-sidebar-conversation-id').element()).toBe(rightSurface)
    expect(mainPanel?.inert).toBe(true)
    expect(rightPanel?.inert).toBe(true)
    expect(mainPanel?.getAttribute('aria-hidden')).toBe('true')
    expect(rightPanel?.getAttribute('aria-hidden')).toBe('true')
    expect(chatSurface.textContent).toBe('conversation-a')
    expect(draftSurface.textContent).toBe('model-2')
    expect(rightSurface.textContent).toBe('conversation-a')
    await expect
      .element(screen.getByTestId('sidebar-active-conversation'))
      .toHaveTextContent('none')
    await expect.element(screen.getByTestId('scheduled-selected')).toHaveTextContent('true')
    await expect.element(screen.getByTestId('scheduled-attention')).toHaveTextContent('4')

    await screen.getByRole('button', { name: 'select-conversation-a' }).click()
    await screen.getByRole('button', { name: 'confirm-external-navigation' }).click()
    await expect
      .poll(() => screen.container.querySelector('[data-testid="scheduled-page-layer"]'))
      .toBeNull()

    expect(mainPanel?.inert).toBe(false)
    expect(rightPanel?.inert).toBe(false)
    expect(mainPanel?.hasAttribute('aria-hidden')).toBe(false)
    expect(rightPanel?.hasAttribute('aria-hidden')).toBe(false)
    expect(chatSurface.textContent).toBe('conversation-a')
    expect(draftSurface.textContent).toBe('model-2')
    expect(rightSurface.textContent).toBe('conversation-a')
    await expect
      .element(screen.getByTestId('sidebar-active-conversation'))
      .toHaveTextContent('conversation-a')
  })

  it('defers sidebar navigation until the scheduled page accepts the external request', async () => {
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'open-scheduled' }).click()
    await screen.getByRole('button', { name: 'new-conversation', exact: true }).click()

    await expect.element(screen.getByTestId('scheduled-page-layer')).toBeVisible()
    await expect
      .element(screen.getByTestId('external-navigation-request-key'))
      .not.toHaveTextContent('none')
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('conversation-a')

    await screen.getByRole('button', { name: 'confirm-external-navigation' }).click()

    await expect
      .poll(() => screen.container.querySelector('[data-testid="scheduled-page-layer"]'))
      .toBeNull()
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('none')
  })

  it('keeps the automation drawer preference for the current app session only', async () => {
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'open-scheduled' }).click()
    await expect
      .element(screen.getByTestId('scheduled-drawer-preferred-width'))
      .toHaveTextContent('440')
    await screen.getByRole('button', { name: 'resize-scheduled-drawer' }).click()
    await expect
      .element(screen.getByTestId('scheduled-drawer-preferred-width'))
      .toHaveTextContent('560')

    await screen.getByRole('button', { name: 'new-conversation', exact: true }).click()
    await screen.getByRole('button', { name: 'confirm-external-navigation' }).click()
    await screen.getByRole('button', { name: 'open-scheduled' }).click()
    await expect
      .element(screen.getByTestId('scheduled-drawer-preferred-width'))
      .toHaveTextContent('560')
  })
})
