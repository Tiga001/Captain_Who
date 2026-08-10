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

it('emits distinct fork points before and after a completed provider transition', async () => {
  const onContinueInNewTask = vi.fn()
  const screen = await render(
    <ChatMessageList
      conversation={conversation()}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      modelTransitionOperations={[
        {
          schemaVersion: 1,
          status: 'completed',
          conversationId: 'forked-task',
          targetModelId: 'model-2',
          operationId: 'transition-1',
          coveredThroughMessageId: 'forked-boundary',
          startedAt: 10,
          completedAt: 20,
          conversationUpdatedAt: 21,
          modelId: 'model-2',
          summaryId: 'summary-1'
        }
      ]}
      onContinueInNewTask={onContinueInNewTask}
      showTokenUsageDetails={false}
    />
  )

  const assistant = document.querySelector<HTMLElement>('[data-message-id="forked-boundary"]')
  const assistantFork = assistant?.querySelector<HTMLButtonElement>(
    'button[aria-label="chat.continueInNewTask"]'
  )
  expect(assistantFork).not.toBeNull()
  assistantFork?.click()
  expect(onContinueInNewTask).toHaveBeenNthCalledWith(1, {
    kind: 'assistant_reply',
    assistantMessageId: 'forked-boundary'
  })

  const divider = screen.getByTestId('model-transition-divider').element()
  const boundaryFork = divider.querySelector<HTMLButtonElement>(
    'button[aria-label="chat.continueInNewTask"]'
  )
  expect(boundaryFork).not.toBeNull()
  boundaryFork?.click()
  expect(onContinueInNewTask).toHaveBeenNthCalledWith(2, {
    kind: 'provider_transition_boundary',
    operationId: 'transition-1'
  })
})
