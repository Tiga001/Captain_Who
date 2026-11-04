import { describe, expect, it } from 'vitest'
import { testState } from './state'
import { mockSuccessfulTurnStarts } from './turnFixtures'
import { emitAgentEvent } from './eventFixtures'
import { renderSelectedConversation } from './setup'

describe('transient LLM retry lifecycle', () => {
  it('flushes final text before readiness without persisting or completing the run for the marker', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    const savesBeforeMarker = testState.saveChatMessageState.mock.calls.length
    const updatedAt = screen.getByTestId('conversation-updated-at').element().textContent
    emitAgentEvent({ type: 'final_answer_ready', runId: 'run-1' })
    await expect.element(screen.getByTestId('final-answer-ready')).toHaveTextContent('true')
    expect(testState.saveChatMessageState).toHaveBeenCalledTimes(savesBeforeMarker)
    expect(screen.getByTestId('conversation-updated-at').element().textContent).toBe(updatedAt)
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('pending')
    emitAgentEvent({
      type: 'model_activity_changed',
      runId: 'run-1',
      streamId: 'final-stream',
      attempt: 1,
      activity: 'waiting'
    })
    await expect.element(screen.getByTestId('final-answer-ready')).toHaveTextContent('false')
    emitAgentEvent({
      type: 'message_delta',
      runId: 'run-1',
      streamId: 'final-stream',
      delta: 'Complete answer before final delivery.'
    })
    emitAgentEvent({ type: 'final_answer_ready', runId: 'run-1' })
    await expect.element(screen.getByTestId('final-answer-ready')).toHaveTextContent('true')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('Complete answer before final delivery.')
    await new Promise((resolve) => window.setTimeout(resolve, 100))
    await expect.element(screen.getByTestId('final-answer-ready')).toHaveTextContent('true')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('pending')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
  })

  it('orders activity after buffered text without persisting activity or touching the conversation', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    const savesBeforeActivity = testState.saveChatMessageState.mock.calls.length
    const updatedAtBeforeActivity = screen
      .getByTestId('conversation-updated-at')
      .element().textContent
    const activity = {
      type: 'model_activity_changed' as const,
      runId: 'run-1',
      streamId: 'stream-1',
      attempt: 1,
      activity: 'waiting' as const
    }
    emitAgentEvent(activity)
    await expect.element(screen.getByTestId('model-activity')).toHaveTextContent('"waiting"')
    emitAgentEvent({ ...activity, activity: 'reasoning' })
    await expect.element(screen.getByTestId('model-activity')).toHaveTextContent('"reasoning"')
    expect(testState.saveChatMessageState).toHaveBeenCalledTimes(savesBeforeActivity)
    expect(screen.getByTestId('conversation-updated-at').element().textContent).toBe(
      updatedAtBeforeActivity
    )

    emitAgentEvent({
      type: 'message_delta',
      runId: activity.runId,
      streamId: activity.streamId,
      delta: 'Narration.'
    })
    emitAgentEvent({ ...activity, activity: 'reasoning' })
    await expect.element(screen.getByTestId('model-activity')).toHaveTextContent('"reasoning"')
    // The activity handler must flush earlier text before applying a later reasoning boundary.
    await new Promise((resolve) => window.setTimeout(resolve, 100))
    await expect.element(screen.getByTestId('model-activity')).toHaveTextContent('"reasoning"')
    emitAgentEvent({
      type: 'message_stream_reset',
      runId: activity.runId,
      streamId: activity.streamId,
      reason: 'retry'
    })
    await expect.element(screen.getByTestId('model-activity')).toHaveTextContent('{}')
    emitAgentEvent({ ...activity, activity: 'reasoning' })
    await expect.element(screen.getByTestId('model-activity')).toHaveTextContent('{}')
  })

  it('updates live state without persisting retryAt into the assistant record', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    const savesBeforeRetry = testState.saveChatMessageState.mock.calls.length

    emitAgentEvent({
      type: 'llm_retry',
      runId: 'run-1',
      streamId: 'stream-1',
      category: 'rate_limited',
      providerCode: 'rate_limit_exceeded',
      delayMs: 5_000,
      retryAt: Date.now() + 5_000,
      attempt: 2,
      maxAttempts: 6
    })

    await expect.element(screen.getByTestId('llm-retry')).toHaveTextContent('"rate_limited"')
    expect(testState.saveChatMessageState).toHaveBeenCalledTimes(savesBeforeRetry)
  })
})
