import { expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ChatMessageList } from '../ChatConversationPage'
import type { ChatConversation } from '../chatTypes'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => key
  })
}))

vi.mock('../../../host/hostClient', () => ({
  hostClient: {}
}))

vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: vi.fn() })
}))

function conversation(): ChatConversation {
  return {
    id: 'forked-task',
    projectId: null,
    modelId: 'model-1',
    title: 'Forked task',
    messages: [
      {
        id: 'forked-user',
        role: 'user',
        content: 'question',
        createdAt: 1,
        status: 'sent'
      },
      {
        id: 'forked-boundary',
        role: 'assistant',
        content: 'inherited answer',
        createdAt: 2,
        status: 'sent'
      },
      {
        id: 'new-user',
        role: 'user',
        content: 'new work',
        createdAt: 3,
        status: 'sent'
      }
    ],
    continuationOrigin: {
      sourceConversationId: 'source-task',
      sourceMessageId: 'source-assistant',
      boundaryMessageId: 'forked-boundary'
    },
    messagesLoaded: true,
    createdAt: 2,
    updatedAt: 3,
    archivedAt: null,
    unreadAt: null
  }
}

it('renders the fork divider immediately after the inherited boundary and opens its source', async () => {
  const onOpenContinuationOrigin = vi.fn()
  const screen = await render(
    <ChatMessageList
      conversation={conversation()}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenContinuationOrigin={onOpenContinuationOrigin}
      showTokenUsageDetails={false}
    />
  )

  const divider = screen.getByTestId('continuation-divider').element()
  expect(divider.previousElementSibling).toHaveAttribute('data-message-id', 'forked-boundary')
  expect(divider.nextElementSibling).toHaveAttribute('data-message-id', 'new-user')

  const button = screen.getByRole('button', { name: 'chat.continuationOrigin' })
  await expect.element(button).toBeVisible()
  const buttonElement = button.element() as HTMLButtonElement
  expect(buttonElement.querySelector('svg')).not.toBeNull()
  buttonElement.click()

  expect(onOpenContinuationOrigin).toHaveBeenCalledWith({
    sourceConversationId: 'source-task',
    sourceMessageId: 'source-assistant',
    boundaryMessageId: 'forked-boundary'
  })
})
