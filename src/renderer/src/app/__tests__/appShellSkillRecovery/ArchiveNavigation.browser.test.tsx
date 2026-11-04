import type {
  AgentConversationTurnOutput,
  AgentConversationTurnRewriteInput,
  AgentProviderTransitionOperation
} from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import { testState } from './state'
import { commitSuccessfulRewrite, mockSuccessfulTurnStarts } from './turnFixtures'
import { emitAgentEvent, completedProviderTransition, deferred } from './eventFixtures'
import { storedConversation, queuedMessage } from './conversationFixtures'
import { AppShell, createComposerDraft, renderSelectedConversation } from './setup'

describe('conversation archive navigation', () => {
  it('leaves the active conversation only after its archive metadata is committed', async () => {
    const archiveSave = deferred<void>()
    testState.saveConversationMeta.mockReturnValueOnce(archiveSave.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()

    await expect.poll(() => testState.saveConversationMeta.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect.element(screen.getByTestId('archived-conversation-a')).toHaveTextContent('false')
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('true')

    const savedConversation = testState.saveConversationMeta.mock.calls[0]?.[0]
    if (!savedConversation) throw new Error('missing archive metadata write')
    expect(savedConversation).toMatchObject({
      id: 'conversation-a',
      archivedAt: null,
      pendingArchivedAt: expect.any(Number),
      unreadAt: null
    })
    testState.persistedConversations.set('conversation-a', {
      ...savedConversation,
      archivedAt: savedConversation.pendingArchivedAt,
      pendingArchivedAt: undefined
    })
    archiveSave.resolve(undefined)

    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    await expect.element(screen.getByTestId('new-conversation-project')).toHaveTextContent('none')
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('none')
    await expect.element(screen.getByTestId('archived-conversation-a')).toHaveTextContent('true')
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('false')
    await expect.poll(() => testState.collaborationRootIds.at(-1)).toBe(null)

    await screen.unmount()
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...storedConversation(), messages: [], messagesLoaded: false, archivedAt: 100 }
    ])
    const reloaded = await render(<AppShell />)
    await expect.element(reloaded.getByTestId('new-conversation-draft')).toBeInTheDocument()
    expect(testState.loadConversation).toHaveBeenCalledTimes(1)
  })

  it('archives a non-active conversation without interrupting the current one', async () => {
    const conversationA = storedConversation()
    const conversationB = {
      ...storedConversation(),
      id: 'conversation-b',
      title: 'Second conversation'
    }
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...conversationA, messages: [], messagesLoaded: false },
      { ...conversationB, messages: [], messagesLoaded: false }
    ])
    testState.persistedConversations.set(conversationB.id, conversationB)

    const screen = await render(<AppShell />)
    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')

    await screen.getByRole('button', { name: 'archive-conversation-b' }).click()

    await expect.element(screen.getByTestId('archived-conversation-b')).toHaveTextContent('true')
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('conversation-a')
    expect(testState.showToast).not.toHaveBeenCalled()
  })

  it('keeps the active conversation selected when archive persistence fails', async () => {
    const persistenceError = new Error('simulated archive failure')
    const failedSave = deferred<void>()
    let firstAttempt = true
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    testState.saveConversationMeta.mockImplementation(async () => {
      if (firstAttempt) {
        firstAttempt = false
        await failedSave.promise
      }
      throw persistenceError
    })
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()

    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect.element(screen.getByTestId('archived-conversation-a')).toHaveTextContent('false')
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('true')

    failedSave.reject(persistenceError)

    await expect.poll(() => testState.showToast.mock.calls.length).toBe(1)
    expect(testState.showToast).toHaveBeenCalledWith('conversation.archiveFailed')
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('false')
    expect(consoleError).toHaveBeenCalledWith(
      'Failed to archive conversation',
      expect.objectContaining({ message: persistenceError.message })
    )
    consoleError.mockRestore()
  })

  it('confirms a committed archive when the save response is lost', async () => {
    const responseLost = new Error('simulated response loss')
    testState.saveConversationMeta.mockImplementationOnce(
      async (conversation: ChatConversation) => {
        testState.persistedConversations.set(conversation.id, {
          ...conversation,
          archivedAt: conversation.pendingArchivedAt,
          pendingArchivedAt: undefined,
          unreadAt: null
        })
        throw responseLost
      }
    )
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()

    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    await expect.element(screen.getByTestId('archived-conversation-a')).toHaveTextContent('true')
    expect(testState.showToast).not.toHaveBeenCalled()
  })

  it('keeps an unknown archive token retryable after a later commit loses readback', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const screen = await renderSelectedConversation()
    let saveAttempt = 0
    testState.saveConversationMeta.mockImplementation(async (conversation: ChatConversation) => {
      saveAttempt += 1
      if (saveAttempt === 2) {
        testState.persistedConversations.set(conversation.id, {
          ...conversation,
          archivedAt: conversation.pendingArchivedAt,
          pendingArchivedAt: undefined,
          unreadAt: null
        })
      }
      if (saveAttempt > 1) throw new Error('response lost')
    })
    let readAttempt = 0
    testState.loadConversationMetas.mockImplementation(async () => {
      readAttempt += 1
      if (readAttempt === 1) {
        return [{ ...storedConversation(), messages: [], messagesLoaded: false }]
      }
      throw new Error('readback unavailable')
    })

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()

    await expect.poll(() => testState.showToast.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('true')

    testState.saveConversationMeta.mockImplementation(async (conversation: ChatConversation) => {
      const current = testState.persistedConversations.get(conversation.id)
      testState.persistedConversations.set(conversation.id, {
        ...current,
        ...conversation,
        archivedAt: conversation.pendingArchivedAt,
        pendingArchivedAt: undefined,
        unreadAt: null
      })
    })
    testState.loadConversationMetas.mockImplementation(async () =>
      [...testState.persistedConversations.values()].map((conversation) => ({
        ...conversation,
        messages: [],
        messagesLoaded: false
      }))
    )
    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()

    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('false')
    expect(testState.persistedConversations.get('conversation-a')?.archivedAt).toEqual(
      expect.any(Number)
    )
    consoleError.mockRestore()
  })

  it('fences a delayed pre-archive metadata save after authoritative readback', async () => {
    const delayedStaleWrite = deferred<void>()
    let delayFirstWrite = true
    testState.saveConversationMeta.mockImplementation(async (conversation: ChatConversation) => {
      if (delayFirstWrite) {
        delayFirstWrite = false
        await delayedStaleWrite.promise
      }
      const current = testState.persistedConversations.get(conversation.id)
      if (current && conversation.updatedAt < current.updatedAt) return
      testState.persistedConversations.set(conversation.id, {
        ...current,
        ...conversation,
        archivedAt: conversation.pendingArchivedAt ?? conversation.archivedAt,
        pendingArchivedAt: undefined,
        unreadAt: conversation.pendingArchivedAt === undefined ? conversation.unreadAt : null
      })
    })
    const staleSave = testState.saveConversationMeta({
      ...storedConversation(),
      title: 'stale pre-archive title'
    })
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    const archivedBeforeStaleCommit = testState.persistedConversations.get('conversation-a')
    expect(archivedBeforeStaleCommit?.archivedAt).toEqual(expect.any(Number))

    delayedStaleWrite.resolve(undefined)
    await staleSave

    const archivedAfterStaleCommit = testState.persistedConversations.get('conversation-a')
    expect(archivedAfterStaleCommit?.archivedAt).toBe(archivedBeforeStaleCommit?.archivedAt)
    expect(archivedAfterStaleCommit?.title).not.toBe('stale pre-archive title')
  })

  it('preserves a concurrent rename when archive and metadata responses settle out of order', async () => {
    const delayedArchiveWrite = deferred<void>()
    let delayFirstWrite = true
    testState.saveConversationMeta.mockImplementation(async (conversation: ChatConversation) => {
      if (delayFirstWrite) {
        delayFirstWrite = false
        await delayedArchiveWrite.promise
      }
      const current = testState.persistedConversations.get(conversation.id)
      if (current && conversation.updatedAt < current.updatedAt) return
      testState.persistedConversations.set(conversation.id, {
        ...current,
        ...conversation,
        archivedAt: conversation.pendingArchivedAt ?? conversation.archivedAt,
        pendingArchivedAt: undefined,
        unreadAt: conversation.pendingArchivedAt === undefined ? conversation.unreadAt : null
      })
    })
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('true')
    await screen.getByRole('button', { name: 'rename-conversation-a' }).click()
    await expect
      .poll(() => testState.persistedConversations.get('conversation-a')?.title)
      .toBe('renamed-conversation-a')

    delayedArchiveWrite.resolve(undefined)

    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    expect(testState.persistedConversations.get('conversation-a')).toMatchObject({
      archivedAt: expect.any(Number),
      title: 'renamed-conversation-a'
    })
    await expect
      .element(screen.getByTestId('title-conversation-a'))
      .toHaveTextContent('renamed-conversation-a')
  })

  it('does not cancel or retire an active Run when navigating away after archive', async () => {
    mockSuccessfulTurnStarts()
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': {
        ...createComposerDraft({ modelId: 'model-1', projectId: 'project-a' }),
        queuedMessages: [queuedMessage('queued-after-archive', 'do not start hidden', 20)]
      }
    })
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    expect(testState.cancelAgentRun).not.toHaveBeenCalled()

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'Finished after archive.'
    })
    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()

    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('queued-after-archive')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.cancelAgentRun).not.toHaveBeenCalled()
  })

  it('does not submit a message when its deferred model transition completes after archive', async () => {
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'compatible',
      reason: 'no_incompatible_history',
      operationId: 'provider-transition-1',
      transitionToken: 'token-model-1'
    })
    const transition =
      deferred<Extract<AgentProviderTransitionOperation, { status: 'completed' }>>()
    testState.startProviderTransition.mockReturnValueOnce(transition.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()

    transition.resolve(completedProviderTransition('model-1'))
    await new Promise((resolve) => window.setTimeout(resolve, 0))

    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
  })

  it('does not reopen history when an atomic edit response arrives after archive', async () => {
    const rewrite = deferred<AgentConversationTurnOutput>()
    testState.rewriteConversationTurn.mockReturnValueOnce(rewrite.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    rewrite.resolve(commitSuccessfulRewrite(input, 99))
    await new Promise((resolve) => window.setTimeout(resolve, 0))

    expect(testState.deleteChatMessages).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.rewriteConversationTurn).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
  })
})
