import type {
  AgentConversationTurnOutput,
  AgentConversationTurnRewriteInput
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import { testState } from './state'
import { latestSkillSelection, commitSuccessfulRewrite } from './turnFixtures'
import { emitAgentEvent, deferred } from './eventFixtures'
import { storedConversation } from './conversationFixtures'
import { createConversationTitle, renderSelectedConversation } from './setup'

describe('edited turn Skill recovery', () => {
  it.each(['running', 'waiting_for_approval', 'waiting_for_user_input'] as const)(
    'rejects edit while the source Turn is %s',
    async (status) => {
      const active = storedConversation()
      const assistant = active.messages.at(-1)
      if (!assistant?.agentRun) throw new Error('missing active edit fixture')
      assistant.status = 'pending'
      assistant.agentRun.status = status
      testState.persistedConversations.set(active.id, active)
      const screen = await renderSelectedConversation()

      await screen.getByRole('button', { name: 'edit-last-message' }).click()
      await new Promise((resolve) => window.setTimeout(resolve, 0))

      expect(testState.rewriteConversationTurn).not.toHaveBeenCalled()
      expect(testState.preflightProviderTransition).not.toHaveBeenCalled()
      await expect
        .element(screen.getByTestId('conversation-message-ids'))
        .toHaveTextContent('user-old,assistant-old')
    }
  )

  it('uses one Agent-domain rewrite without sending or copying conversation history', async () => {
    const longConversation = storedConversation()
    longConversation.messages = [
      ...Array.from({ length: 100 }, (_, index) => [
        {
          id: `history-user-${index}`,
          role: 'user' as const,
          content: `history-user-content-${index}`,
          createdAt: index * 2 + 10,
          status: 'sent' as const
        },
        {
          id: `history-assistant-${index}`,
          role: 'assistant' as const,
          content: `history-assistant-content-${index}`,
          createdAt: index * 2 + 11,
          status: 'sent' as const
        }
      ]).flat(),
      ...longConversation.messages
    ]
    testState.persistedConversations.set(longConversation.id, longConversation)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    expect(input.requestId).toMatch(/^conversation-turn-rewrite-request-/)
    expect(input.sourceUserMessageId).toBe('user-old')
    expect(input.sourceAssistantMessageId).toBe('assistant-old')
    expect(input.turn.conversationId).toBe('conversation-a')
    expect(input.turn.content).toBe('edited')
    expect(JSON.stringify(input)).not.toContain('history-user-content-0')
    expect(testState.deleteChatMessages).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .not.toHaveTextContent('user-old')
  })

  it('keeps the current model and replaces only an automatic title without a Provider transition', async () => {
    const automaticTitleConversation = storedConversation()
    automaticTitleConversation.title = createConversationTitle(
      automaticTitleConversation.messages[0]!.content,
      'chat.newConversation'
    )
    testState.persistedConversations.set(automaticTitleConversation.id, automaticTitleConversation)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')
    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    expect(input.turn.modelId).toBe('model-1')
    expect(input.turn.title).toBe(createConversationTitle('edited', 'chat.newConversation'))
    expect(testState.preflightProviderTransition).not.toHaveBeenCalled()
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('title-conversation-a'))
      .toHaveTextContent(createConversationTitle('edited', 'chat.newConversation'))
  })

  it('keeps the prior Timeline unchanged until the rewrite is durably accepted', async () => {
    const rewrite = deferred<AgentConversationTurnOutput>()
    testState.rewriteConversationTurn.mockReturnValueOnce(rewrite.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .toHaveTextContent('user-old,assistant-old')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('original|done')

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    rewrite.resolve(commitSuccessfulRewrite(input, 101))

    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
  })

  it('keeps the original Timeline and never creates an error pair when rewrite is rejected', async () => {
    testState.rewriteConversationTurn.mockRejectedValueOnce(new Error('rewrite rejected'))
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)

    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .toHaveTextContent('user-old,assistant-old')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('original|done')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.deleteChatMessages).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
  })

  it('recovers a committed rewrite when the first authoritative reload fails', async () => {
    const screen = await renderSelectedConversation()
    const reload = deferred<ChatConversation | null>()
    testState.loadConversation.mockReturnValueOnce(reload.promise)

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('original|done')

    reload.reject(new Error('transient reload failure'))
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
    expect(testState.loadConversation.mock.calls.length).toBeGreaterThanOrEqual(3)

    emitAgentEvent({
      type: 'done',
      runId: 'run-100',
      success: true,
      status: 'completed',
      content: 'rewritten answer'
    })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('rewritten answer')
  })

  it('falls back to the committed output pair and preserves event routing when reload stays down', async () => {
    const screen = await renderSelectedConversation()
    testState.loadConversation
      .mockRejectedValueOnce(new Error('reload unavailable 1'))
      .mockRejectedValueOnce(new Error('reload unavailable 2'))

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .not.toHaveTextContent('user-old')

    emitAgentEvent({
      type: 'done',
      runId: 'run-100',
      success: true,
      status: 'completed',
      content: 'fallback answer'
    })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('fallback answer')
  })

  it('keeps a terminal rewrite response settled when authoritative reload stays down', async () => {
    let replacementAssistantId = ''
    testState.rewriteConversationTurn.mockImplementationOnce(
      async (input: AgentConversationTurnRewriteInput) => {
        const output = commitSuccessfulRewrite(input, 106)
        replacementAssistantId = output.assistantMessageId
        const conversation = testState.persistedConversations.get(output.conversationId)
        const assistant = conversation?.messages.find(
          (message) => message.id === output.assistantMessageId
        )
        if (!conversation || !assistant?.agentRun) {
          throw new Error('missing terminal rewrite fixture')
        }
        assistant.content = 'rewrite launch failed'
        assistant.status = 'error'
        assistant.agentRun.status = 'failed'
        assistant.agentRun.completedAt = 30
        output.assistantMessage = structuredClone(assistant)
        return output
      }
    )
    const screen = await renderSelectedConversation()
    testState.loadConversation
      .mockRejectedValueOnce(new Error('reload unavailable 1'))
      .mockRejectedValueOnce(new Error('reload unavailable 2'))

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('error')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('failed')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('rewrite launch failed')
    expect(
      testState.saveChatMessageState.mock.calls.some(
        (call) =>
          (call[1] as ChatConversation['messages'][number]).id === replacementAssistantId &&
          (call[1] as ChatConversation['messages'][number]).status === 'pending'
      )
    ).toBe(false)

    emitAgentEvent({
      type: 'done',
      runId: 'run-106',
      success: true,
      status: 'completed',
      content: 'must remain ignored'
    })
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('failed')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .not.toHaveTextContent('must remain ignored')
  })

  it('deduplicates concurrent edit submission before it reaches the Host', async () => {
    const rewrite = deferred<AgentConversationTurnOutput>()
    testState.rewriteConversationTurn.mockReturnValueOnce(rewrite.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    rewrite.resolve(commitSuccessfulRewrite(input, 102))
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
    expect(testState.rewriteConversationTurn).toHaveBeenCalledTimes(1)
  })

  it('keeps next-turn settings and new input queued before an edited turn is acknowledged', async () => {
    const rewrite = deferred<AgentConversationTurnOutput>()
    testState.rewriteConversationTurn.mockReturnValueOnce(rewrite.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()
    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()
    await screen.getByRole('button', { name: 'select-latest-skill' }).click()
    await screen.getByRole('button', { name: 'append-queued-message' }).click()

    rewrite.resolve(commitSuccessfulRewrite(input, 107))
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
    expect(input.turn.modelId).toBe('model-1')
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent ?? '{}')).toEqual(
      expect.objectContaining({
        modelId: 'model-2',
        permissionMode: 'custom',
        message: 'new draft after failure',
        skills: [latestSkillSelection]
      })
    )
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-late')
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
  })

  it('keeps navigation on another conversation when a background rewrite settles', async () => {
    const conversationB = {
      ...storedConversation(),
      id: 'conversation-b',
      title: 'Second conversation'
    }
    testState.persistedConversations.set(conversationB.id, conversationB)
    const rewrite = deferred<AgentConversationTurnOutput>()
    testState.rewriteConversationTurn.mockReturnValueOnce(rewrite.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'select-conversation-b', exact: true }).click()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-b')

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    rewrite.resolve(commitSuccessfulRewrite(input, 103))
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-b')

    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
  })

  it('does not reopen an archived conversation when its committed rewrite response arrives', async () => {
    const rewrite = deferred<AgentConversationTurnOutput>()
    testState.rewriteConversationTurn.mockReturnValueOnce(rewrite.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    rewrite.resolve(commitSuccessfulRewrite(input, 104))
    await new Promise((resolve) => window.setTimeout(resolve, 0))

    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    expect(testState.rewriteConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
  })

  it('reuses the idempotency key when the same rejected edit is retried', async () => {
    let firstAttempt: AgentConversationTurnRewriteInput | null = null
    let committedOutput: AgentConversationTurnOutput | null = null
    testState.rewriteConversationTurn.mockImplementation(
      async (input: AgentConversationTurnRewriteInput) => {
        if (!firstAttempt) {
          firstAttempt = structuredClone(input)
          committedOutput = commitSuccessfulRewrite(input, 105)
          throw new Error('transport interrupted')
        }
        if (JSON.stringify(input) !== JSON.stringify(firstAttempt)) {
          throw new Error('idempotency identity conflict')
        }
        if (!committedOutput) throw new Error('missing committed rewrite output')
        return committedOutput
      }
    )
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(2)

    const first = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    const second = testState.rewriteConversationTurn.mock
      .calls[1]?.[0] as AgentConversationTurnRewriteInput
    expect(second).toEqual(first)
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
  })
})
