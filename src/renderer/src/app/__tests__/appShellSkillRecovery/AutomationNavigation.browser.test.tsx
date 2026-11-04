import type { AgentEvent, AutomationEvent, AutomationOpenRequest } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatConversation, ChatMessage } from '../../../features/chat/chatTypes'
import { testState } from './state'
import { deferred } from './eventFixtures'
import { storedConversation } from './conversationFixtures'
import { AppShell, renderSelectedConversation } from './setup'

describe('automation conversation navigation', () => {
  it('refreshes conversation metadata when an Automation run creates a new chat', async () => {
    const screen = await render(<AppShell />)
    await expect
      .element(screen.getByRole('button', { name: 'select-conversation-a' }))
      .toBeVisible()
    await expect.poll(() => testState.automationEventListeners.size).toBe(1)
    const initialMetaLoadCount = testState.loadConversationMetas.mock.calls.length
    const automationConversation: ChatConversation = {
      ...storedConversation(),
      id: 'automation-conversation',
      title: 'Automation output',
      updatedAt: 20
    }
    testState.persistedConversations.set(automationConversation.id, automationConversation)

    const event: AutomationEvent = {
      schemaVersion: 1,
      sequence: 1,
      eventId: 'automation-event-1',
      kind: 'run_updated',
      automationId: 'automation-1',
      runId: 'automation-run-1',
      resourceRevision: 1,
      occurredAt: 20
    }
    for (const listener of testState.automationEventListeners) listener(event)

    await expect
      .poll(() => testState.loadConversationMetas.mock.calls.length)
      .toBe(initialMetaLoadCount + 1)
    await expect
      .element(screen.getByRole('button', { name: 'select-automation-conversation' }))
      .toBeVisible()
    expect(testState.loadConversation).not.toHaveBeenCalledWith('automation-conversation')
    await expect
      .element(screen.getByTestId('sidebar-active-conversation'))
      .toHaveTextContent('none')
  })

  it('reloads an open Automation chat and binds events that arrived before its run projection', async () => {
    const screen = await renderSelectedConversation()
    await expect.poll(() => testState.automationEventListeners.size).toBe(1)
    const initialPendingActionLoads = testState.listPendingAgentActions.mock.calls.length
    const stored = storedConversation()
    const automationAssistant: ChatMessage = {
      id: 'assistant-automation',
      role: 'assistant',
      content: '',
      createdAt: 20,
      status: 'pending',
      agentRun: {
        runId: 'automation-agent-run',
        status: 'running',
        toolDefinitions: [],
        toolCalls: [],
        toolResults: [],
        approvals: [],
        fileChangeProposals: [],
        timeline: []
      }
    }
    testState.persistedConversations.set(stored.id, {
      ...stored,
      messages: [...stored.messages, automationAssistant],
      updatedAt: 20
    })

    const agentEvent: AgentEvent = {
      type: 'message',
      runId: 'automation-agent-run',
      content: 'live automation answer'
    }
    for (const listener of testState.agentEventListeners) listener(agentEvent)
    const automationEvent: AutomationEvent = {
      schemaVersion: 1,
      sequence: 2,
      eventId: 'automation-event-2',
      kind: 'run_updated',
      automationId: 'automation-1',
      runId: 'automation-run-1',
      resourceRevision: 2,
      occurredAt: 20
    }
    for (const listener of testState.automationEventListeners) listener(automationEvent)

    await expect
      .poll(() => testState.loadConversation.mock.calls.filter(([id]) => id === stored.id).length)
      .toBeGreaterThan(1)
    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .toHaveTextContent('assistant-automation')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('live automation answer')
    await expect
      .poll(() => testState.listPendingAgentActions.mock.calls.length)
      .toBeGreaterThan(initialPendingActionLoads)
  })

  it('hydrates a conversation missing from the local catalog before opening its target message', async () => {
    const screen = await render(<AppShell />)
    await expect
      .element(screen.getByRole('button', { name: 'select-conversation-a' }))
      .toBeVisible()
    await expect.poll(() => testState.automationOpenRequestListeners.size).toBe(1)
    const automationConversation: ChatConversation = {
      ...storedConversation(),
      id: 'automation-conversation',
      title: 'Automation output',
      updatedAt: 20
    }
    testState.persistedConversations.set(automationConversation.id, automationConversation)

    const openRequest: AutomationOpenRequest = {
      schemaVersion: 1,
      automationId: 'automation-1',
      runId: 'automation-run-1',
      destination: {
        kind: 'conversation',
        conversationId: automationConversation.id,
        messageId: 'assistant-old'
      }
    }
    for (const listener of testState.automationOpenRequestListeners) listener(openRequest)

    await expect
      .poll(() => testState.loadConversation.mock.calls)
      .toContainEqual([automationConversation.id])
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent(automationConversation.id)
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent(automationConversation.id)
    await expect
      .element(screen.getByTestId('scroll-target-message-id'))
      .toHaveTextContent('assistant-old')
    await expect
      .element(screen.getByRole('button', { name: 'select-automation-conversation' }))
      .toBeVisible()
  })

  it('keeps the scheduled page mounted while a native chat request is guarded and hydrated', async () => {
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'open-scheduled' }).click()
    await expect.poll(() => testState.automationOpenRequestListeners.size).toBe(1)
    const automationConversation: ChatConversation = {
      ...storedConversation(),
      id: 'automation-conversation',
      title: 'Automation output',
      updatedAt: 20
    }
    const detail = deferred<ChatConversation | undefined>()
    testState.loadConversation.mockImplementationOnce(() => detail.promise)

    const openRequest: AutomationOpenRequest = {
      schemaVersion: 1,
      automationId: 'automation-1',
      runId: 'automation-run-1',
      destination: {
        kind: 'conversation',
        conversationId: automationConversation.id,
        messageId: 'assistant-old'
      }
    }
    for (const listener of testState.automationOpenRequestListeners) listener(openRequest)

    await expect.element(screen.getByTestId('scheduled-page-layer')).toBeVisible()
    expect(testState.loadConversation).not.toHaveBeenCalledWith(automationConversation.id)
    await screen.getByRole('button', { name: 'confirm-external-navigation' }).click()
    await expect
      .poll(() => testState.loadConversation.mock.calls)
      .toContainEqual([automationConversation.id])
    await expect.element(screen.getByTestId('scheduled-page-layer')).toBeVisible()

    detail.resolve(automationConversation)
    await expect
      .poll(() => screen.container.querySelector('[data-testid="scheduled-page-layer"]'))
      .toBeNull()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent(automationConversation.id)
    await expect
      .element(screen.getByTestId('scroll-target-message-id'))
      .toHaveTextContent('assistant-old')
  })

  it('does not let a stale native chat load override a newer scheduled task intent', async () => {
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'open-scheduled' }).click()
    await expect.poll(() => testState.automationOpenRequestListeners.size).toBe(1)
    const detail = deferred<ChatConversation | undefined>()
    testState.loadConversation.mockImplementationOnce(() => detail.promise)
    const conversationRequest: AutomationOpenRequest = {
      schemaVersion: 1,
      automationId: 'automation-1',
      runId: 'automation-run-1',
      destination: {
        kind: 'conversation',
        conversationId: 'automation-conversation',
        messageId: 'assistant-old'
      }
    }
    for (const listener of testState.automationOpenRequestListeners) listener(conversationRequest)
    await screen.getByRole('button', { name: 'confirm-external-navigation' }).click()
    await expect
      .poll(() => testState.loadConversation.mock.calls)
      .toContainEqual(['automation-conversation'])

    const taskRequest: AutomationOpenRequest = {
      schemaVersion: 1,
      automationId: 'automation-2',
      runId: 'automation-run-2',
      destination: { kind: 'task' }
    }
    for (const listener of testState.automationOpenRequestListeners) listener(taskRequest)
    detail.resolve({
      ...storedConversation(),
      id: 'automation-conversation',
      updatedAt: 20
    })

    await expect
      .element(screen.getByRole('button', { name: 'select-automation-conversation' }))
      .toBeVisible()
    await expect.element(screen.getByTestId('scheduled-page-layer')).toBeVisible()
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect
      .element(screen.getByTestId('sidebar-active-conversation'))
      .toHaveTextContent('none')
  })
})
