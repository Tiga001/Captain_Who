import { HostInvocationError } from '@mycopilot/host-api'
import type {
  AgentConversationTurnContinueInput,
  AgentConversationTurnOutput
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { testState } from './state'
import {
  emitAgentEvent,
  emitProviderTransition,
  runningProviderTransition,
  deferred
} from './eventFixtures'
import { storedConversation, queuedMessage } from './conversationFixtures'
import { createComposerDraft, renderSelectedConversation } from './setup'

describe('explicit stopped-task continuation', () => {
  function prepareStoppedTask() {
    const stored = storedConversation()
    const assistant = stored.messages.at(-1)!
    assistant.content = ''
    assistant.agentRun = {
      ...assistant.agentRun!,
      status: 'cancelled',
      userInterrupted: true,
      completedAt: 3
    }
    testState.persistedConversations.set(stored.id, stored)
    return stored
  }
  function continuationOutput(
    input: AgentConversationTurnContinueInput,
    sequence: number
  ): AgentConversationTurnOutput {
    const stored = testState.persistedConversations.get(input.conversationId)!
    const user = stored.messages.findLast((message) => message.role === 'user')!
    return {
      runId: `resume-run-${sequence}`,
      eventName: 'agent.event',
      conversationId: stored.id,
      userMessageId: user.id,
      assistantMessageId: input.assistantMessageId,
      userMessage: { ...user, role: 'user' },
      assistantMessage: {
        id: input.assistantMessageId,
        role: 'assistant',
        content: '',
        createdAt: 30 + sequence,
        status: 'pending'
      },
      activatedSkills: []
    }
  }
  it('restores a durable stop, resumes without a user bubble, and supports stopping again', async () => {
    prepareStoppedTask()
    let sequence = 0
    testState.continueConversationTurn.mockImplementation(
      async (input: AgentConversationTurnContinueInput) => continuationOutput(input, ++sequence)
    )
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'resume-task' }).click()
    await expect.poll(() => testState.continueConversationTurn.mock.calls.length).toBe(1)
    const input = testState.continueConversationTurn.mock
      .calls[0][0] as AgentConversationTurnContinueInput
    expect(input.sourceAssistantMessageId).toBe('assistant-old')
    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .toHaveTextContent(`user-old,assistant-old,${input.assistantMessageId}`)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    emitAgentEvent({
      type: 'done',
      runId: 'resume-run-1',
      status: 'cancelled',
      success: false,
      userInterrupted: true,
      content: ''
    })
    await screen.getByRole('button', { name: 'resume-task' }).click()
    await expect.poll(() => testState.continueConversationTurn.mock.calls.length).toBe(2)
    expect(testState.continueConversationTurn.mock.calls[1][0].sourceAssistantMessageId).toBe(
      input.assistantMessageId
    )
    emitAgentEvent({
      type: 'done',
      runId: 'resume-run-2',
      status: 'completed',
      success: true,
      content: 'finished'
    })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    expect(screen.container.textContent).not.toContain('resume-task')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
  })
  it('keeps queued messages untouched and rejects duplicate clicks while admission is pending', async () => {
    prepareStoppedTask()
    testState.loadComposerDrafts.mockResolvedValue({
      'conversation-a': createComposerDraft({
        modelId: 'model-1',
        projectId: 'project-a',
        queuedMessages: [queuedMessage('queued-keep', 'keep queued', 5)]
      })
    })
    const admitted = deferred<AgentConversationTurnOutput>()
    testState.continueConversationTurn.mockReturnValue(admitted.promise)
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'resume-task' }).click()
    await expect.poll(() => testState.continueConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('resume-starting')).toHaveTextContent('true')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    const input = testState.continueConversationTurn.mock
      .calls[0][0] as AgentConversationTurnContinueInput
    admitted.resolve(continuationOutput(input, 1))
    await expect.element(screen.getByTestId('resume-starting')).toHaveTextContent('false')
    emitAgentEvent({
      type: 'done',
      runId: 'resume-run-1',
      status: 'completed',
      success: true,
      content: 'finished'
    })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(screen.container.textContent).toContain('keep queued')
    expect(testState.continueConversationTurn).toHaveBeenCalledTimes(1)
  })
  it('settles a completed continuation replay without waiting for another done event', async () => {
    prepareStoppedTask()
    testState.continueConversationTurn.mockImplementationOnce(
      async (input: AgentConversationTurnContinueInput) => {
        const output = continuationOutput(input, 1)
        return {
          ...output,
          assistantMessage: {
            ...output.assistantMessage,
            status: 'sent',
            content: 'already finished'
          }
        }
      }
    )
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'resume-task' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('already finished')
    expect(screen.container.textContent).not.toContain('resume-task')
    expect(testState.continueConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
  })

  it('reconciles a lost resume acknowledgement without resubmitting or duplicating the original user message', async () => {
    const stored = prepareStoppedTask()
    testState.continueConversationTurn.mockImplementationOnce(
      async (input: AgentConversationTurnContinueInput) => {
        const output = continuationOutput(input, 1)
        testState.persistedConversations.set(stored.id, {
          ...stored,
          messages: [
            ...stored.messages,
            {
              ...output.assistantMessage,
              role: 'assistant',
              status: 'pending',
              agentRun: {
                ...stored.messages.at(-1)!.agentRun!,
                userInterrupted: undefined,
                runId: output.runId,
                status: 'running',
                completedAt: undefined
              }
            }
          ]
        })
        throw new Error('lost response')
      }
    )
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'resume-task' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    const input = testState.continueConversationTurn.mock
      .calls[0][0] as AgentConversationTurnContinueInput
    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .toHaveTextContent(`user-old,assistant-old,${input.assistantMessageId}`)
    expect(testState.continueConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
  })

  it('resumes after a confirmed model transition and preserves newer composer input', async () => {
    prepareStoppedTask()
    const running = runningProviderTransition('model-1')
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'requires_compaction',
      reason: 'provider_protocol_changed',
      operationId: running.operationId,
      transitionToken: 'resume-transition'
    })
    testState.startProviderTransition.mockResolvedValueOnce(running)
    testState.continueConversationTurn.mockImplementation(
      async (input: AgentConversationTurnContinueInput) => continuationOutput(input, 1)
    )
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'resume-task' }).click()
    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()
    emitProviderTransition({
      ...running,
      status: 'completed',
      modelId: 'model-1',
      summaryId: 'resume-summary',
      completedAt: running.startedAt + 10,
      conversationUpdatedAt: running.startedAt + 11
    })
    await expect.poll(() => testState.continueConversationTurn.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('draft-message'))
      .toHaveTextContent('new draft after failure')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
  })

  it('removes only the new assistant after a certified admission rejection and permits retry', async () => {
    prepareStoppedTask()
    testState.continueConversationTurn.mockRejectedValueOnce(
      new HostInvocationError({
        code: -32000,
        message: 'Continuation refused',
        data: { continuationAdmissionRejected: true }
      })
    )
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'resume-task' }).click()
    await expect.poll(() => testState.continueConversationTurn.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .toHaveTextContent('user-old,assistant-old')
    await expect.element(screen.getByRole('button', { name: 'resume-task' })).toBeEnabled()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.showToast).toHaveBeenCalledWith('chat.resumeFailed')
  })

  it('does not offer continuation for cancellations without an explicit user stop', async () => {
    const stored = prepareStoppedTask()
    delete stored.messages.at(-1)!.agentRun!.userInterrupted
    const screen = await renderSelectedConversation()
    expect(screen.container.textContent).not.toContain('resume-task')
    expect(testState.continueConversationTurn).not.toHaveBeenCalled()
  })
})
