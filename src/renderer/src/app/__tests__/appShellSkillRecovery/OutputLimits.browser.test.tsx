import type { AgentConversationTurnRewriteInput } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { testState } from './state'
import { mockSuccessfulTurnStarts } from './turnFixtures'
import { renderSelectedConversation } from './setup'

describe('provider-owned output limits', () => {
  it('does not impose maxTokens on a newly submitted turn', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0]).not.toHaveProperty('maxTokens')
  })

  it('does not impose maxTokens when editing a message and regenerating the reply', async () => {
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    expect(input.turn).not.toHaveProperty('maxTokens')
  })
})
