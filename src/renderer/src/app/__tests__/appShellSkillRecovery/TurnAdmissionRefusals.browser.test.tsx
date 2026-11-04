import { HostInvocationError } from '@mycopilot/host-api'
import { describe, expect, it } from 'vitest'
import { testState } from './state'
import { createComposerDraft, renderSelectedConversation } from './setup'

describe('trusted Main turn admission refusals', () => {
  it.each(['ACCOUNT_LOGIN_REQUIRED', 'ACCOUNT_LICENSE_REQUIRED', 'ACCOUNT_LICENSE_UNAVAILABLE'])(
    'keeps composer input and existing history without a failed turn for %s',
    async (code) => {
      testState.loadComposerDrafts.mockResolvedValueOnce({
        'conversation-a': createComposerDraft({
          modelId: 'model-1',
          message: 'Preserve this draft'
        })
      })
      testState.startConversationTurn.mockRejectedValueOnce(
        new HostInvocationError({ message: code })
      )
      const screen = await renderSelectedConversation()
      const previousIds = screen.getByTestId('conversation-message-ids').element().textContent
      await screen.getByRole('button', { name: 'submit-draft-content' }).click()
      await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
      await expect
        .poll(() => screen.getByTestId('conversation-message-ids').element().textContent)
        .toBe(previousIds)
      await expect
        .element(screen.getByTestId('draft-message'))
        .toHaveTextContent('Preserve this draft')
      expect(testState.cancelAgentRun).not.toHaveBeenCalled()
    }
  )

  it.each(['ACCOUNT_LOGIN_REQUIRED', 'ACCOUNT_LICENSE_REQUIRED', 'ACCOUNT_LICENSE_UNAVAILABLE'])(
    'preserves the original edited turn when rewrite is refused with %s',
    async (code) => {
      testState.rewriteConversationTurn.mockRejectedValueOnce(
        new HostInvocationError({ message: code })
      )
      const screen = await renderSelectedConversation()
      const previousIds = screen.getByTestId('conversation-message-ids').element().textContent
      const previousContent = screen
        .getByTestId('conversation-message-contents')
        .element().textContent
      await screen.getByRole('button', { name: 'edit-last-message' }).click()
      await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
      await expect
        .poll(() => screen.getByTestId('conversation-message-ids').element().textContent)
        .toBe(previousIds)
      await expect
        .element(screen.getByTestId('conversation-message-contents'))
        .toHaveTextContent(previousContent ?? '')
      expect(testState.cancelAgentRun).not.toHaveBeenCalled()
    }
  )
})
