import { describe, expect, it } from 'vitest'
import { testState } from './state'
import { mockSuccessfulTurnStarts } from './turnFixtures'
import { emitAgentEvent } from './eventFixtures'
import { AppShell, createComposerDraft, renderSelectedConversation } from './setup'

describe('organization sidebar migration', () => {
  it('keeps a covered conversation unread and exposes its sidebar attention while the organization is maximized', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'open-workflows', exact: true }).click()
    testState.rightOpen = true
    testState.rightMaximized = true
    await screen.rerender(<AppShell />)
    await expect
      .element(screen.getByTestId('sidebar-active-conversation'))
      .toHaveTextContent('none')
    expect(screen.container.querySelector<HTMLElement>('.main-panel')?.inert).toBe(true)
    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'Completed while watching the workflow.'
    })
    await expect.element(screen.getByTestId('unread-conversation-a')).toHaveTextContent('true')
  })

  it('opens the global organization home maximized without replacing the chat or composer draft', async () => {
    testState.loadComposerDrafts.mockResolvedValue({
      'conversation-a': createComposerDraft({
        message: 'Keep my current draft',
        modelId: 'model-1',
        projectId: 'project-a'
      })
    })
    const screen = await renderSelectedConversation()
    const chat = screen.getByTestId('active-conversation-id').element()
    const draft = screen.getByTestId('draft-message').element()
    await screen.getByRole('button', { name: 'open-workflows', exact: true }).click()
    await expect
      .element(screen.getByTestId('workflow-route'))
      .toHaveTextContent('"instanceId":null')
    expect(testState.openRightSidebar).toHaveBeenCalled()
    expect(testState.setRightSidebarMaximized).toHaveBeenLastCalledWith(true)
    expect(screen.container.querySelector('.workflow-page-layer')).toBeNull()
    expect(screen.getByTestId('active-conversation-id').element()).toBe(chat)
    expect(screen.getByTestId('draft-message').element()).toBe(draft)
    expect(draft.textContent).toBe('Keep my current draft')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
  })

  it('restores the conversation area when leaving scheduled after opening a maximized organization', async () => {
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'open-workflows', exact: true }).click()
    const workflowRoute = screen.getByTestId('workflow-route').element().textContent
    await screen.getByRole('button', { name: 'open-scheduled', exact: true }).click()
    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await screen.getByRole('button', { name: 'confirm-external-navigation' }).click()
    expect(testState.setRightSidebarMaximized).toHaveBeenLastCalledWith(false)
    expect(screen.getByTestId('workflow-route').element().textContent).toBe(workflowRoute)
    await expect.element(screen.getByTestId('scheduled-selected')).toHaveTextContent('false')
    await screen.getByRole('button', { name: 'open-workflows', exact: true }).click()
    await screen.getByRole('button', { name: 'new-conversation', exact: true }).click()
    expect(testState.setRightSidebarMaximized).toHaveBeenLastCalledWith(false)
  })
})
