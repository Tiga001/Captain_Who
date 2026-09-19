import { expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ChatMessageList } from '../ChatConversationPage'
import type { ChatConversation } from '../chatTypes'
import type { AgentManualContextCompactionOperation } from '@mycopilot/protocol'

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

it('keeps the clicked reply fork spinning while the parent disables fork entry points', async () => {
  let resolve!: () => void
  const onContinueInNewTask = vi.fn(
    () =>
      new Promise<void>((done) => {
        resolve = done
      })
  )
  const props = {
    conversation: conversation(),
    editableLastUserMessageId: null,
    editSelectedModelAvailable: true,
    editSelectedModelSupportsImage: true,
    onContinueInNewTask,
    showTokenUsageDetails: false
  }
  const screen = await render(<ChatMessageList {...props} />)
  const button = screen.getByRole('button', { name: 'chat.continueInNewTask', exact: true })
  await button.click()
  await screen.rerender(
    <ChatMessageList {...props} forkDisabledReason="chat.continueInNewTaskPending" />
  )
  await expect.element(button).toBeVisible()
  await expect.element(button).toBeDisabled()
  expect(button.element().querySelector('.chat-message__action-spinner')).not.toBeNull()
  ;(button.element() as HTMLButtonElement).click()
  expect(onContinueInNewTask).toHaveBeenCalledExactlyOnceWith({
    kind: 'assistant_reply',
    assistantMessageId: 'forked-boundary'
  })

  resolve()
  await screen.rerender(<ChatMessageList {...props} />)
  await expect.element(button).toBeEnabled()
  expect(button.element().querySelector('.chat-message__action-spinner')).toBeNull()
})

const manualOperation: AgentManualContextCompactionOperation = {
  schemaVersion: 1,
  operationId: 'context-compaction-cloned',
  requestId: 'request',
  conversationId: 'forked-task',
  status: 'completed',
  phase: 'committing',
  startedAt: 10,
  updatedAt: 20,
  completedAt: 20,
  coveredThroughMessageId: 'forked-boundary',
  summaryId: 'summary'
}

it('forks an older manual divider by operation identity and suppresses repeated clicks', async () => {
  let resolve!: () => void
  const onContinueInNewTask = vi.fn(
    () =>
      new Promise<void>((done) => {
        resolve = done
      })
  )
  const screen = await render(
    <ChatMessageList
      conversation={conversation()}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      manualCompactionOperations={[manualOperation]}
      onContinueInNewTask={onContinueInNewTask}
      showTokenUsageDetails={false}
    />
  )
  const divider = screen.getByTestId('manual-compaction-divider').element()
  expect(divider.nextElementSibling).toHaveAttribute('data-message-id', 'new-user')
  const button = divider.querySelector<HTMLButtonElement>('.conversation-model-transition__fork')!
  button.click()
  button.click()
  expect(onContinueInNewTask).toHaveBeenCalledExactlyOnceWith({
    kind: 'manual_compaction_boundary',
    operationId: 'context-compaction-cloned'
  })
  await expect.element(button).toBeDisabled()
  resolve()
  await expect.element(button).toBeEnabled()
})

it('keeps the manual fork disabled while busy and absent in observer mode', async () => {
  const onContinueInNewTask = vi.fn()
  const props = {
    conversation: conversation(),
    editableLastUserMessageId: null,
    editSelectedModelAvailable: true,
    editSelectedModelSupportsImage: true,
    manualCompactionOperations: [manualOperation],
    onContinueInNewTask,
    showTokenUsageDetails: false
  }
  const screen = await render(<ChatMessageList {...props} forkDisabledReason="聊天空闲时可用" />)
  const button = screen
    .getByTestId('manual-compaction-divider')
    .element()
    .querySelector<HTMLButtonElement>('.conversation-model-transition__fork')!
  await expect.element(button).toBeDisabled()
  button.click()
  expect(onContinueInNewTask).not.toHaveBeenCalled()
  await screen.rerender(<ChatMessageList {...props} mode="observer" />)
  expect(
    screen
      .getByTestId('manual-compaction-divider')
      .element()
      .querySelector('.conversation-model-transition__fork')
  ).toBeNull()
})
