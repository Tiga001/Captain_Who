import type { AgentConversationTurnInput, AutomationEvent } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { Activity } from 'react'
import { render } from 'vitest-browser-react'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import { testState } from './state'
import { skillSelection, mockSuccessfulTurnStarts } from './turnFixtures'
import {
  emitAgentEvent,
  emitProviderTransition,
  completedProviderTransition,
  runningProviderTransition,
  deferred
} from './eventFixtures'
import {
  storedConversation,
  storedConversationWithRun,
  queuedMessage
} from './conversationFixtures'
import {
  AppShell,
  createComposerDraft,
  defaultUiPreferences,
  renderSelectedConversation
} from './setup'
import { loadQueue, finishRun } from './queueFixtures'

describe('running conversation guidance queue', () => {
  it('keeps a running turn on its settings and applies composer choices to the next turn', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    const firstTurn = structuredClone(testState.startConversationTurn.mock.calls[0]?.[0])
    const preflightCalls = testState.preflightProviderTransition.mock.calls.length
    const transitionCalls = testState.startProviderTransition.mock.calls.length

    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()
    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0]).toEqual(firstTurn)
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(preflightCalls)
    expect(testState.startProviderTransition).toHaveBeenCalledTimes(transitionCalls)
    expect(testState.steerAgentRun).not.toHaveBeenCalled()
    expect(testState.cancelAgentRun).not.toHaveBeenCalled()

    finishRun('run-1')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await screen.getByRole('button', { name: 'submit-draft-content' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    expect(testState.startConversationTurn.mock.calls[1]?.[0]).toEqual(
      expect.objectContaining({
        content: 'new draft after failure',
        modelId: 'model-2',
        permissions: defaultUiPreferences().customPermissions
      })
    )
  })

  it('auto-sends the frozen queue settings without replacing newer composer choices', async () => {
    const queued = queuedMessage('queue-older-settings', 'queued on the first model', 10)
    loadQueue([queued])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    const firstTurn = testState.startConversationTurn.mock.calls[0]?.[0]
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()
    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()

    finishRun('run-1')
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    expect(testState.startConversationTurn.mock.calls[1]?.[0]).toEqual(
      expect.objectContaining({
        content: queued.content,
        modelId: queued.modelId,
        permissions: firstTurn.permissions
      })
    )
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent ?? '{}')).toEqual(
      expect.objectContaining({
        modelId: 'model-2',
        permissionMode: 'custom',
        message: 'new draft after failure'
      })
    )
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')

    finishRun('run-2')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await screen.getByRole('button', { name: 'submit-draft-content' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(3)
    expect(testState.startConversationTurn.mock.calls[2]?.[0]).toEqual(
      expect.objectContaining({
        content: 'new draft after failure',
        modelId: 'model-2',
        permissions: defaultUiPreferences().customPermissions
      })
    )
  })

  it('keeps newer composer choices when a queued message finishes its confirmed model transition', async () => {
    loadQueue([queuedMessage('queue-transition-settings', 'queued before model change', 10)])
    const running = runningProviderTransition('model-1')
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'requires_compaction',
      reason: 'provider_protocol_changed',
      operationId: running.operationId,
      transitionToken: 'transition-token-queue-settings'
    })
    testState.startProviderTransition.mockResolvedValueOnce(running)
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()

    emitProviderTransition({
      ...running,
      status: 'completed',
      modelId: 'model-1',
      summaryId: 'summary-queue-settings',
      completedAt: running.startedAt + 10,
      conversationUpdatedAt: running.startedAt + 11
    })
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0]).toEqual(
      expect.objectContaining({ content: 'queued before model change', modelId: 'model-1' })
    )
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent ?? '{}')).toEqual(
      expect.objectContaining({ modelId: 'model-2', permissionMode: 'custom' })
    )
  })

  it('leaves the queue untouched by default when a reply completes', async () => {
    loadQueue([queuedMessage('queue-first', 'keep for later', 10)])
    const screen = await renderSelectedConversation()
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    finishRun('run-1')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-first')
  })

  it('starts an idle queue immediately and sends exactly one row per completed reply', async () => {
    loadQueue([
      queuedMessage('queue-first', 'first', 30),
      queuedMessage('queue-second', 'second', 20),
      queuedMessage('queue-third', 'third', 10)
    ])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0].content).toBe('first')
    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('queue-second,queue-third')

    finishRun('run-1')
    finishRun('run-1')
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe('second')
    finishRun('run-1')
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-third')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(2)

    finishRun('run-2')
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(3)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn.mock.calls[2]?.[0].content).toBe('third')
    finishRun('run-3')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(3)
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
  })

  it('waits for the authoritative terminal read without a Renderer state write before the next queued message', async () => {
    loadQueue([
      queuedMessage('queue-first', 'first', 10),
      queuedMessage('queue-second', 'after durable settlement', 20)
    ])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput
    const terminal = storedConversationWithRun(input.assistantMessageId!, 'run-1', 'completed')
    const terminalRead = deferred<ChatConversation>()
    testState.loadConversation.mockReturnValueOnce(terminalRead.promise)
    testState.saveChatMessageState.mockImplementation(() => new Promise<void>(() => undefined))
    const readsBeforeDone = testState.loadConversation.mock.calls.length

    finishRun('run-1')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect.poll(() => testState.loadConversation.mock.calls.length).toBe(readsBeforeDone + 1)
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-second')

    terminalRead.resolve(terminal)
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(2)
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe(
      'after durable settlement'
    )
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
  })

  it('preserves enabled queue execution across effect teardown and restoration', async () => {
    loadQueue([
      queuedMessage('queue-first', 'first before effect teardown', 10),
      queuedMessage('queue-second', 'continue after effect restoration', 20)
    ])
    // Effect restoration rehydrates drafts; reflect the writes instead of returning the
    // suite's empty default draft on that second storage read.
    testState.loadComposerDrafts.mockImplementation(async () => ({
      'conversation-a': testState.saveComposerDraft.mock.calls.findLast(
        ([scopeId]) => scopeId === 'conversation-a'
      )?.[1]
    }))
    const shell = (mode: 'visible' | 'hidden') => (
      <Activity mode={mode}>
        <AppShell />
      </Activity>
    )
    const screen = await render(shell('visible'))
    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    await screen.rerender(shell('hidden'))
    await expect.poll(() => testState.agentEventListeners.size).toBe(0)
    await screen.rerender(shell('visible'))
    await expect.poll(() => testState.agentEventListeners.size).toBe(1)
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('true')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-second')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    finishRun('run-1')
    await expect
      .poll(() => ({
        starts: testState.startConversationTurn.mock.calls.length,
        preflights: testState.preflightProviderTransition.mock.calls.length,
        status: screen.getByTestId('agent-run-status').element().textContent,
        enabled: screen.getByTestId('queue-auto-send-enabled').element().textContent
      }))
      .toEqual({ starts: 2, preflights: 2, status: 'running', enabled: 'true' })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe(
      'continue after effect restoration'
    )
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
  })

  it('honors the current drag order while retaining each queued message payload', async () => {
    const next = {
      ...queuedMessage('queue-second', 'send with attached context', 20),
      modelId: 'model-2',
      permissionMode: 'custom' as const,
      skills: [skillSelection],
      attachments: [
        {
          id: 'attachment-queue-order',
          kind: 'file' as const,
          name: 'context.txt',
          mimeType: 'text/plain',
          sizeBytes: 6,
          encoding: 'managed' as const,
          data: 'managed-test-cXVldWVk'
        }
      ]
    }
    loadQueue([queuedMessage('queue-first', 'send last', 10), next])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await screen.getByRole('button', { name: 'reverse-queued-messages' }).click()
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-2',
      decision: 'compatible',
      reason: 'no_incompatible_history',
      operationId: 'queued-context-model-transition',
      transitionToken: 'queued-context-model-token'
    })
    testState.startProviderTransition.mockResolvedValueOnce(
      completedProviderTransition('model-2', { operationId: 'queued-context-model-transition' })
    )
    finishRun('run-1')

    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    expect(testState.startConversationTurn.mock.calls[1]?.[0]).toEqual(
      expect.objectContaining({
        content: next.content,
        modelId: next.modelId,
        permissions: defaultUiPreferences().customPermissions,
        projectId: next.projectId,
        skills: next.skills,
        attachments: next.attachments
      })
    )
    expect(testState.startProviderTransition).toHaveBeenLastCalledWith({
      conversationId: 'conversation-a',
      targetModelId: 'model-2',
      transitionToken: 'queued-context-model-token'
    })
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-first')
  })

  it('sends a message added after completion already observed an empty enabled queue', async () => {
    loadQueue([queuedMessage('queue-first', 'first', 10)])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
    finishRun('run-1')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('true')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    await screen.getByRole('button', { name: 'append-queued-message' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe(
      'message queued after completion'
    )
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
  })

  it('waits for completed Done after an active-run block instead of polling preflight', async () => {
    loadQueue([queuedMessage('queue-next', 'send after the ready signal', 10)])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    emitAgentEvent({
      type: 'state',
      runId: 'run-1',
      state: { status: 'completed', activeRunId: null, lastError: null, updatedAt: 30 }
    })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'blocked',
      reason: 'active_run',
      message: 'The turn has not published its ready signal yet.'
    })
    vi.useFakeTimers()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await vi.advanceTimersByTimeAsync(0)
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(2)
    await vi.advanceTimersByTimeAsync(10_000)
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(2)
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    finishRun('run-1')
    await vi.advanceTimersByTimeAsync(0)
    vi.useRealTimers()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe(
      'send after the ready signal'
    )
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
  })

  it('resumes an enabled queue when authoritative storage refresh restores a completed reply', async () => {
    const running = storedConversationWithRun('assistant-restored', 'run-restored', 'running')
    testState.persistedConversations.set(running.id, running)
    loadQueue([queuedMessage('queue-next', 'send after completed state is restored', 10)])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()

    const completed = {
      ...storedConversationWithRun('assistant-restored', 'run-restored', 'completed'),
      updatedAt: running.updatedAt + 100
    }
    const refresh = deferred<ChatConversation>()
    testState.persistedConversations.set(completed.id, completed)
    testState.loadConversation.mockReturnValueOnce(refresh.promise)
    const loadsBeforeRefresh = testState.loadConversation.mock.calls.length
    const event: AutomationEvent = {
      schemaVersion: 1,
      sequence: 1,
      eventId: 'completed-state-refresh',
      kind: 'run_updated',
      automationId: 'automation-refresh',
      runId: 'automation-refresh-run',
      resourceRevision: 1,
      occurredAt: completed.updatedAt
    }
    for (const listener of testState.automationEventListeners) listener(event)
    await expect
      .poll(() => testState.loadConversation.mock.calls.length)
      .toBe(loadsBeforeRefresh + 1)
    expect(testState.startConversationTurn).not.toHaveBeenCalled()

    refresh.resolve(completed)
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn.mock.calls[0]?.[0].content).toBe(
      'send after completed state is restored'
    )
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
  })

  it('does not retry past a pending-approval preflight block', async () => {
    loadQueue([queuedMessage('queue-first', 'wait for approval', 10)])
    testState.preflightProviderTransition.mockResolvedValue({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'blocked',
      reason: 'pending_approval',
      message: 'A user approval is still pending.'
    })
    const screen = await renderSelectedConversation()
    vi.useFakeTimers()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await vi.advanceTimersByTimeAsync(10_000)
    vi.useRealTimers()

    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(1)
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-first')
  })

  it('disabling auto-send preserves the remaining queue and does not cancel the active reply', async () => {
    loadQueue([
      queuedMessage('queue-first', 'start now', 10),
      queuedMessage('queue-second', 'keep for later', 20)
    ])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.cancelAgentRun).not.toHaveBeenCalled()
    finishRun('run-1')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-second')
  })

  it.each(['waiting_for_approval', 'waiting_for_user_input'] as const)(
    'keeps auto-send enabled but waits for the same run to finish while %s',
    async (status) => {
      loadQueue([
        queuedMessage('queue-first', 'needs approval', 10),
        queuedMessage('queue-second', 'after approval and reply', 20)
      ])
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
      emitAgentEvent({ type: 'done', runId: 'run-1', success: true, status })
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent(status)
      await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('true')
      expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
      await expect
        .element(screen.getByTestId('queued-message-ids'))
        .toHaveTextContent('queue-second')

      emitAgentEvent({ type: 'started', runId: 'run-1', toolDefinitions: [] })
      finishRun('run-1')
      await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
      expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe(
        'after approval and reply'
      )
    }
  )

  it.each(['failed', 'cancelled'] as const)(
    'pauses the queue after a %s run until explicitly re-enabled',
    async (status) => {
      loadQueue([
        queuedMessage('queue-first', 'first', 10),
        queuedMessage('queue-second', 'retry manually', 20)
      ])
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
      finishRun('run-1', status)
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent(status)
      await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
      await expect
        .element(screen.getByTestId('queued-message-ids'))
        .toHaveTextContent('queue-second')
      expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

      await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
      await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
      expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe('retry manually')
    }
  )

  it('pauses auto-send immediately when Stop is clicked before cancellation is acknowledged', async () => {
    loadQueue([
      queuedMessage('queue-first', 'first', 10),
      queuedMessage('queue-second', 'keep after stop', 20)
    ])
    const cancellation = deferred<boolean>()
    testState.cancelAgentRun.mockReturnValueOnce(cancellation.promise)
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
    expect(testState.cancelAgentRun).toHaveBeenCalledWith('run-1')
    finishRun('run-1')
    cancellation.resolve(true)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-second')
  })

  it.each([false, true])(
    'discards a stale preflight after disabling auto-send (re-enable: %s)',
    async (reEnable) => {
      loadQueue([queuedMessage('queue-first', 'send only from the current switch session', 10)])
      const preflight = deferred<{
        conversationId: string
        targetModelId: string
        decision: 'compatible'
        reason: 'same_protocol'
        operationId: string
        transitionToken: string
      }>()
      testState.preflightProviderTransition.mockReturnValueOnce(preflight.promise)
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
      await expect.poll(() => testState.preflightProviderTransition.mock.calls.length).toBe(1)
      await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
      if (reEnable) await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
      preflight.resolve({
        conversationId: 'conversation-a',
        targetModelId: 'model-1',
        decision: 'compatible',
        reason: 'same_protocol',
        operationId: 'stale-queue-preflight',
        transitionToken: 'stale-queue-token'
      })

      if (reEnable) {
        await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
        await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
        expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(2)
      } else {
        await new Promise((resolve) => window.setTimeout(resolve, 0))
        await expect
          .element(screen.getByTestId('queue-auto-send-enabled'))
          .toHaveTextContent('false')
        expect(testState.startConversationTurn).not.toHaveBeenCalled()
        await expect
          .element(screen.getByTestId('queued-message-ids'))
          .toHaveTextContent('queue-first')
      }
      expect(testState.startProviderTransition).not.toHaveBeenCalledWith(
        expect.objectContaining({ transitionToken: 'stale-queue-token' })
      )
    }
  )

  it('keeps the switch scoped to its conversation and never sends a different conversation queue', async () => {
    const other = { ...storedConversation(), id: 'conversation-b' }
    testState.persistedConversations.set(other.id, other)
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({
        modelId: 'model-1',
        projectId: 'project-a',
        queuedMessages: [queuedMessage('queue-a', 'send A', 10)]
      }),
      'conversation-b': createComposerDraft({
        modelId: 'model-1',
        projectId: 'project-a',
        queuedMessages: [queuedMessage('queue-b', 'keep B', 10)]
      })
    })
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'select-conversation-b', exact: true }).click()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-b')
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
    finishRun('run-1')
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-b')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('true')
  })

  it('uses the latest queue head after reordering during a running Provider transition', async () => {
    loadQueue([
      queuedMessage('queue-first', 'send last', 10),
      { ...queuedMessage('queue-second', 'send first after transition', 20), modelId: 'model-2' }
    ])
    const running = runningProviderTransition('model-1')
    testState.preflightProviderTransition
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-1',
        decision: 'requires_compaction',
        reason: 'provider_protocol_changed',
        operationId: running.operationId,
        transitionToken: 'reordered-queue-transition'
      })
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-2',
        decision: 'compatible',
        reason: 'no_incompatible_history',
        operationId: 'reordered-next-model-transition',
        transitionToken: 'reordered-next-model-token'
      })
    testState.startProviderTransition
      .mockResolvedValueOnce(running)
      .mockResolvedValueOnce(
        completedProviderTransition('model-2', { operationId: 'reordered-next-model-transition' })
      )
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('provider_protocol_changed')
    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect
      .element(screen.getByTestId('model-transition-operations'))
      .toHaveTextContent(`${running.operationId}:running`)
    await screen.getByRole('button', { name: 'reverse-queued-messages' }).click()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()

    emitProviderTransition({
      ...running,
      status: 'completed',
      modelId: 'model-1',
      completedAt: 40,
      conversationUpdatedAt: 50
    })
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0]).toEqual(
      expect.objectContaining({ modelId: 'model-2', content: 'send first after transition' })
    )
    expect(testState.startProviderTransition).toHaveBeenCalledTimes(2)
    expect(testState.startProviderTransition).toHaveBeenLastCalledWith({
      conversationId: 'conversation-a',
      targetModelId: 'model-2',
      transitionToken: 'reordered-next-model-token'
    })
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-first')
  })
})
