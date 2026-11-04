import type {
  AgentConversationTurnInput,
  AgentProviderTransitionOperation
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { testState } from './state'
import { skillSelection, mockSuccessfulTurnStarts } from './turnFixtures'
import {
  emitProviderTransition,
  completedProviderTransition,
  runningProviderTransition,
  failedProviderTransition,
  deferred
} from './eventFixtures'
import { createComposerDraft, renderSelectedConversation } from './setup'

describe('provider transition guard', () => {
  it('keeps newer next-turn choices and input when an earlier confirmed transition completes', async () => {
    mockSuccessfulTurnStarts()
    const running = runningProviderTransition('model-1')
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'requires_compaction',
      reason: 'provider_protocol_changed',
      operationId: running.operationId,
      transitionToken: 'transition-token-older-composer'
    })
    testState.startProviderTransition.mockResolvedValueOnce(running)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()
    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()

    emitProviderTransition({
      ...running,
      status: 'completed',
      modelId: 'model-1',
      summaryId: 'summary-older-composer',
      completedAt: running.startedAt + 10,
      conversationUpdatedAt: running.startedAt + 11
    })
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0]).toEqual(
      expect.objectContaining({ content: 'follow up', modelId: 'model-1' })
    )
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent ?? '{}')).toEqual(
      expect.objectContaining({
        modelId: 'model-2',
        permissionMode: 'custom',
        message: 'new draft after failure'
      })
    )
  })

  it('preserves next-turn choices made while a compatible transition reply is pending', async () => {
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'compatible',
      reason: 'no_incompatible_history',
      operationId: 'provider-transition-1',
      transitionToken: 'token-model-1'
    })
    mockSuccessfulTurnStarts()
    const transition =
      deferred<Extract<AgentProviderTransitionOperation, { status: 'completed' }>>()
    testState.startProviderTransition.mockReturnValueOnce(transition.promise)
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()

    transition.resolve(completedProviderTransition('model-1'))
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0].modelId).toBe('model-1')
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent ?? '{}')).toEqual(
      expect.objectContaining({ modelId: 'model-2', permissionMode: 'custom' })
    )
  })

  it('uses the completed transition model when the submitted composer selection is unchanged', async () => {
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'compatible',
      reason: 'no_incompatible_history',
      operationId: 'provider-transition-1',
      transitionToken: 'token-model-1'
    })
    mockSuccessfulTurnStarts()
    testState.startProviderTransition.mockImplementationOnce(async () => ({
      ...completedProviderTransition('model-1'),
      modelId: 'model-2'
    }))
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0].modelId).toBe('model-2')
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')
  })

  it('keeps model selection in the draft and preflights only when the user sends', async () => {
    mockSuccessfulTurnStarts()
    const running = runningProviderTransition('model-2')
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-2',
      decision: 'requires_compaction',
      reason: 'api_provider_changed',
      operationId: running.operationId,
      transitionToken: 'transition-token-model-2'
    })
    testState.startProviderTransition.mockResolvedValueOnce(running)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')
    expect(testState.preflightProviderTransition).not.toHaveBeenCalled()
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.saveConversationMeta).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    await expect.poll(() => testState.saveComposerDraft.mock.calls.length).toBeGreaterThan(0)

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.preflightProviderTransition.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('api_provider_changed')
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('model-transition-operations'))
      .toHaveTextContent(`${running.operationId}:running`)
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')

    const conversationUpdatedAt = running.startedAt + 11
    emitProviderTransition({
      ...running,
      status: 'completed',
      modelId: 'model-2',
      summaryId: 'summary-model-2',
      completedAt: running.startedAt + 10,
      conversationUpdatedAt
    })
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')
    await expect
      .poll(() => Number(screen.getByTestId('conversation-updated-at').element().textContent))
      .toBeGreaterThanOrEqual(conversationUpdatedAt)
    await expect
      .poll(() => Number(screen.getByTestId('draft-updated-at').element().textContent))
      .toBeGreaterThanOrEqual(conversationUpdatedAt)
    await expect.poll(() => screen.getByTestId('draft-message').element().textContent).toBe('')
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(
      (testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput).modelId
    ).toBe('model-2')
  })

  it('preflights a same-model submit before creating messages and resumes it after confirmation', async () => {
    mockSuccessfulTurnStarts()
    const running = runningProviderTransition('model-1')
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'requires_compaction',
      reason: 'provider_protocol_changed',
      operationId: running.operationId,
      transitionToken: 'transition-token-same-model'
    })
    testState.startProviderTransition.mockResolvedValueOnce(running)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.preflightProviderTransition.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('provider_protocol_changed')

    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn).not.toHaveBeenCalled()

    emitProviderTransition({
      ...running,
      status: 'completed',
      modelId: 'model-1',
      summaryId: 'summary-same-model',
      completedAt: running.startedAt + 10,
      conversationUpdatedAt: running.startedAt + 11
    })
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(
      (testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput).content
    ).toBe('follow up')
  })

  it('keeps the full composer payload and target model when transition confirmation is cancelled', async () => {
    const retainedDraft = createComposerDraft({
      attachments: [
        {
          id: 'attachment-retained',
          kind: 'file',
          name: 'retained.txt',
          mimeType: 'text/plain',
          sizeBytes: 8,
          encoding: 'managed',
          data: 'managed-test-cmV0YWluZWQ='
        }
      ],
      message: 'keep this draft',
      modelId: 'model-1',
      permissionMode: 'custom',
      projectId: 'project-a',
      skills: [skillSelection]
    })
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': retainedDraft
    })
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'requires_compaction',
      reason: 'provider_protocol_changed',
      operationId: 'provider-transition-cancelled',
      transitionToken: 'transition-token-cancelled'
    })
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-with-skill' }).click()
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('provider_protocol_changed')
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(1)
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('provider_protocol_changed')
    await screen.getByRole('button', { name: 'cancel-model-transition' }).click()

    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent ?? '{}')).toEqual({
      attachments: retainedDraft.attachments,
      message: retainedDraft.message,
      modelId: 'model-2',
      permissionMode: retainedDraft.permissionMode,
      projectId: retainedDraft.projectId,
      skills: retainedDraft.skills
    })
  })

  it('keeps the original model after failure and switches only after an explicit retry succeeds', async () => {
    const running = runningProviderTransition('model-2')
    const completedRetry = completedProviderTransition('model-2', {
      coveredThroughMessageId: 'assistant-old',
      summaryId: 'summary-retry'
    })
    testState.preflightProviderTransition
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-2',
        decision: 'requires_compaction',
        reason: 'api_provider_changed',
        operationId: running.operationId,
        transitionToken: 'transition-token-model-2'
      })
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-2',
        decision: 'requires_compaction',
        reason: 'api_provider_changed',
        operationId: completedRetry.operationId,
        transitionToken: 'transition-token-model-2-retry'
      })
    testState.startProviderTransition
      .mockResolvedValueOnce(failedProviderTransition(running))
      .mockResolvedValueOnce(completedRetry)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect
      .element(screen.getByTestId('model-transition-operations'))
      .toHaveTextContent(`${running.operationId}:failed`)
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')

    await screen.getByRole('button', { name: 'retry-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')
  })

  it('does not replay a failed composer intent after the user edits the draft and retries', async () => {
    mockSuccessfulTurnStarts()
    const running = runningProviderTransition('model-1')
    const completedRetry = completedProviderTransition('model-1', {
      coveredThroughMessageId: 'assistant-old',
      summaryId: 'summary-retry'
    })
    testState.preflightProviderTransition
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-1',
        decision: 'requires_compaction',
        reason: 'provider_protocol_changed',
        operationId: running.operationId,
        transitionToken: 'transition-token-submit'
      })
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-1',
        decision: 'requires_compaction',
        reason: 'provider_protocol_changed',
        operationId: completedRetry.operationId,
        transitionToken: 'transition-token-retry'
      })
    testState.startProviderTransition
      .mockResolvedValueOnce(failedProviderTransition(running))
      .mockResolvedValueOnce(completedRetry)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect
      .element(screen.getByTestId('model-transition-operations'))
      .toHaveTextContent(`${running.operationId}:failed`)

    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()
    await expect
      .element(screen.getByTestId('draft-message'))
      .toHaveTextContent('new draft after failure')

    await screen.getByRole('button', { name: 'retry-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(2)
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('draft-message'))
      .toHaveTextContent('new draft after failure')
  })

  it('restores historical dividers without replaying an old completed switch on reload', async () => {
    const older = completedProviderTransition('model-1', {
      operationId: 'operation-old',
      coveredThroughMessageId: 'assistant-before-old',
      summaryId: 'summary-old',
      startedAt: 10,
      completedAt: 20
    })
    const newer = completedProviderTransition('model-2', {
      operationId: 'operation-new',
      coveredThroughMessageId: 'assistant-before-new',
      summaryId: 'summary-new',
      startedAt: 30,
      completedAt: 40
    })
    testState.getProviderTransitionStatus.mockResolvedValueOnce({ operations: [newer, older] })
    const screen = await renderSelectedConversation()

    await expect
      .element(screen.getByTestId('model-transition-operations'))
      .toHaveTextContent('operation-old:completed,operation-new:completed')
    // The persisted conversation/draft already contains the authoritative current model. A later
    // compatible transition may not have a compaction receipt, so loading old receipts must not
    // roll it back to the target of the latest historical compaction.
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-1')
  })

  it('rewrites on the current model without compacting or replacing source history first', async () => {
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'requires_compaction',
      reason: 'provider_protocol_changed',
      operationId: 'provider-transition-edit',
      transitionToken: 'transition-token-edit'
    })
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    expect(testState.preflightProviderTransition).not.toHaveBeenCalled()
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    expect(testState.deleteChatMessages).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .not.toHaveTextContent('provider_protocol_changed')
  })
})
