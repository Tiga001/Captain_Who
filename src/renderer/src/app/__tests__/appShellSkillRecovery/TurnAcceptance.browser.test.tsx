import { HostInvocationError } from '@mycopilot/host-api'
import type { AgentConversationTurnInput, AgentConversationTurnOutput } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { testState } from './state'
import { successfulTurnOutput, mockSuccessfulTurnStarts } from './turnFixtures'
import { emitAgentEvent, deferred } from './eventFixtures'
import { storedConversation } from './conversationFixtures'
import { createComposerDraft, renderSelectedConversation } from './setup'

describe('Host-owned turn acceptance', () => {
  const submittedAttachment = {
    id: 'submitted-attachment',
    kind: 'file' as const,
    name: 'question.txt',
    mimeType: 'text/plain',
    sizeBytes: 8,
    encoding: 'managed' as const,
    data: 'managed-test-cXVlc3Rpb24='
  }

  function prepareDraft() {
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({
        message: 'read this file',
        attachments: [submittedAttachment],
        modelId: 'model-1',
        projectId: 'project-a'
      })
    })
  }

  it('keeps the draft and timeline unchanged until lightweight send preflight returns', async () => {
    prepareDraft()
    mockSuccessfulTurnStarts()
    const preflight = deferred<{
      conversationId: string
      targetModelId: string
      decision: 'ready_for_send'
      reason: 'same_protocol'
    }>()
    testState.preflightProviderTransition.mockReturnValueOnce(preflight.promise)
    const screen = await renderSelectedConversation()
    const previousIds = screen.getByTestId('conversation-message-ids').element().textContent
    await screen.getByRole('button', { name: 'submit-draft-content' }).click()
    await expect.poll(() => testState.preflightProviderTransition.mock.calls.length).toBe(1)
    expect(testState.preflightProviderTransition).toHaveBeenCalledWith({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      forSend: true
    })
    expect(screen.getByTestId('conversation-message-ids').element().textContent).toBe(previousIds)
    await expect.element(screen.getByTestId('draft-message')).toHaveTextContent('read this file')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    preflight.resolve({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'ready_for_send',
      reason: 'same_protocol'
    })
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0].attachments).toEqual([
      submittedAttachment
    ])
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    await expect.element(screen.getByTestId('draft-message')).toHaveTextContent('')
  })

  it('keeps optimistic messages local and lets Host admission own the turn metadata', async () => {
    prepareDraft()
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-draft-content' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('starting')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('read this file')
    await expect.element(screen.getByTestId('draft-message')).toHaveTextContent('')
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(1)
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    expect(testState.saveConversationMeta).not.toHaveBeenCalled()
    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput
    expect(input.attachments).toEqual([submittedAttachment])
    start.resolve(successfulTurnOutput(input, 1))

    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    expect(testState.saveConversationMeta).not.toHaveBeenCalled()
    await expect.element(screen.getByTestId('agent-run-projection')).toHaveTextContent('run-1')
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
  })

  it('waits for queued metadata from an unconfirmed send before the next single send preflight', async () => {
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    const metadata = deferred<void>()
    testState.saveConversationMeta.mockReturnValueOnce(metadata.promise)
    start.reject(new Error('admission response unavailable'))
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('failed')
    await expect.poll(() => testState.saveConversationMeta.mock.calls.length).toBe(1)
    mockSuccessfulTurnStarts()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    metadata.resolve()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(2)
    expect(testState.saveConversationMeta).toHaveBeenCalledTimes(1)
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
  })

  it('persists the rejected local pair with its input and attachments without overwriting a newer draft', async () => {
    prepareDraft()
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-draft-content' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()
    start.reject(new Error('Host rejected the turn before starting a Run'))

    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('failed')
    await expect.poll(() => testState.upsertChatMessages.mock.calls.length).toBe(1)
    expect(testState.saveConversationMeta).toHaveBeenCalled()
    expect(testState.saveConversationMeta.mock.invocationCallOrder[0]).toBeLessThan(
      testState.upsertChatMessages.mock.invocationCallOrder[0]
    )
    // A transport failure can hide a successful Host start. Never send an unbound
    // failed state over its authoritative assistant; the insert-only fallback is safe.
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages.mock.calls[0]).toEqual([
      'conversation-a',
      [
        expect.objectContaining({
          role: 'user',
          content: 'read this file',
          attachments: [expect.objectContaining({ id: submittedAttachment.id })]
        }),
        expect.objectContaining({
          role: 'assistant',
          status: 'sent',
          agentRun: expect.objectContaining({ runId: null, status: 'failed' })
        })
      ],
      2
    ])
    await expect
      .element(screen.getByTestId('draft-message'))
      .toHaveTextContent('new draft after failure')
  })

  it('restores rejected input and queues newer input with auto-send disabled', async () => {
    prepareDraft()
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()
    const previousIds = screen.getByTestId('conversation-message-ids').element().textContent
    await screen.getByRole('button', { name: 'submit-draft-content' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('draft-message')).toHaveTextContent('')
    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    start.reject(new HostInvocationError({ message: 'ACCOUNT_LICENSE_UNAVAILABLE' }))
    await expect.element(screen.getByTestId('draft-message')).toHaveTextContent('read this file')
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
    expect(screen.getByTestId('conversation-message-ids').element().textContent).toBe(previousIds)
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent!)).toMatchObject({
      modelId: 'model-1',
      attachments: [submittedAttachment]
    })
    expect(
      JSON.parse(screen.getByTestId('queued-message-payloads').element().textContent!)
    ).toEqual([
      expect.objectContaining({
        content: 'new draft after failure',
        modelId: 'model-2',
        status: 'pending'
      })
    ])
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
  })

  it.each(['running', 'completed'] as const)(
    'recovers a lost acceptance response for a %s turn without resending',
    async (status) => {
      prepareDraft()
      const start = deferred<AgentConversationTurnOutput>()
      testState.startConversationTurn.mockReturnValueOnce(start.promise)
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'submit-draft-content' }).click()
      await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
      const input = testState.startConversationTurn.mock.calls[0][0] as AgentConversationTurnInput
      const output = successfulTurnOutput(input, 1)
      const stored = storedConversation()
      testState.persistedConversations.set(stored.id, {
        ...stored,
        messages: [
          ...stored.messages,
          { ...output.userMessage, status: 'sent' },
          {
            ...output.assistantMessage,
            content: status === 'completed' ? 'Backend completed the answer' : '',
            status: status === 'completed' ? 'sent' : 'pending',
            agentRun: {
              runId: output.runId,
              status,
              toolDefinitions: [],
              toolCalls: [],
              toolResults: [],
              approvals: [],
              fileChangeProposals: [],
              timeline: []
            }
          }
        ]
      })
      const reads = testState.loadConversation.mock.calls.length
      start.reject(new Error('Lost IPC response'))
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent(status)
      expect(testState.loadConversation.mock.calls.length).toBeGreaterThan(reads)
      expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
      expect(testState.upsertChatMessages).not.toHaveBeenCalled()
      if (status === 'running') {
        emitAgentEvent({
          type: 'done',
          runId: 'run-1',
          success: true,
          status: 'completed',
          content: 'Recovered stream'
        })
        await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
      } else {
        await expect
          .element(screen.getByTestId('conversation-message-contents'))
          .toHaveTextContent('Backend completed the answer')
      }
    }
  )

  it('keeps an unconfirmed stopped start local instead of overwriting Host state', async () => {
    prepareDraft()
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-draft-content' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    start.reject(new Error('The start response was lost'))

    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('cancelled')
    await expect.poll(() => testState.upsertChatMessages.mock.calls.length).toBe(1)
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages.mock.calls[0]?.[1]).toEqual([
      expect.objectContaining({ role: 'user', content: 'read this file' }),
      expect.objectContaining({
        role: 'assistant',
        agentRun: expect.objectContaining({ runId: null, status: 'cancelled' })
      })
    ])
  })
})
