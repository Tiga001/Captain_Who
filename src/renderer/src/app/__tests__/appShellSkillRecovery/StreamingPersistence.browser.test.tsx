import { describe, expect, it, vi } from 'vitest'
import type { ChatMessage } from '../../../features/chat/chatTypes'
import { testState } from './state'
import { mockSuccessfulTurnStarts } from './turnFixtures'
import { emitAgentEvent } from './eventFixtures'
import { storedConversationWithRun } from './conversationFixtures'
import { renderSelectedConversation } from './setup'

describe('Core-owned streaming message persistence', () => {
  it('upgrades a legacy run on Started after Core installs its checkpoint generation', async () => {
    const stored = storedConversationWithRun('assistant-legacy', 'run-legacy', 'running')
    testState.persistedConversations.set(stored.id, stored)
    const screen = await renderSelectedConversation()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    emitAgentEvent({ type: 'started', runId: 'run-legacy', toolDefinitions: [] })
    emitAgentEvent({
      type: 'message',
      runId: 'run-legacy',
      content: 'Resumed with Core ownership.'
    })
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('Resumed with Core ownership.')
    window.dispatchEvent(new Event('pagehide'))
    await new Promise((resolve) => window.setTimeout(resolve, 350))
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
  })

  it.each([false, true])(
    'restores a running projection with Core ownership %s while retaining legacy saves',
    async (coreOwned) => {
      const stored = storedConversationWithRun('assistant-restored', 'run-restored', 'running')
      const assistant = stored.messages.at(-1)!
      if (coreOwned)
        assistant.agentRun!.coreCheckpoint = {
          generation: '11111111-1111-4111-8111-111111111111',
          sequence: 2,
          fingerprint: 'a'.repeat(64)
        }
      testState.persistedConversations.set(stored.id, stored)
      const screen = await renderSelectedConversation()
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
      emitAgentEvent({
        type: 'message_delta',
        runId: 'run-restored',
        streamId: 'restored-stream',
        delta: 'Recovered live text.'
      })
      await expect
        .element(screen.getByTestId('conversation-message-contents'))
        .toHaveTextContent('Recovered live text.')
      window.dispatchEvent(new Event('pagehide'))
      if (coreOwned) {
        await new Promise((resolve) => window.setTimeout(resolve, 350))
        expect(testState.saveChatMessageState).not.toHaveBeenCalled()
      } else {
        await expect.poll(() => testState.saveChatMessageState.mock.calls.length).toBeGreaterThan(0)
        expect(
          (testState.saveChatMessageState.mock.calls.at(-1)?.[1] as ChatMessage).content
        ).toContain('Recovered live text.')
      }
    }
  )

  it('keeps 80ms rendering with no Renderer serialization or save RPC through terminal state', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()

    vi.useFakeTimers()
    for (let index = 1; index <= 9; index += 1) {
      emitAgentEvent({
        type: 'message_delta',
        runId: 'run-1',
        streamId: 'stream-1',
        delta: String(index)
      })
      await vi.advanceTimersByTimeAsync(80)
    }

    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('123456789')
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    expect(testState.saveConversationMeta).not.toHaveBeenCalled()

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: '123456789'
    })
    await vi.advanceTimersByTimeAsync(0)
    vi.useRealTimers()

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    expect(testState.saveConversationMeta).not.toHaveBeenCalled()
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
  })
})
