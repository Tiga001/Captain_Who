import { expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatMessage } from '../chatTypes'
import { ChatMessageItem } from '../components/ChatMessageItem'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => key
  })
}))

vi.mock('../../storage/storageClient', () => ({
  loadAttachmentImage: vi.fn(),
  loadImageFile: vi.fn(),
  revealStoredProjectFile: vi.fn()
}))

vi.mock('../../../host/hostClient', () => ({
  hostClient: {}
}))

vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: vi.fn() })
}))

function userMessage(favorited = false): ChatMessage {
  return {
    id: 'user-1',
    role: 'user',
    content: 'Remember this request',
    createdAt: 1,
    status: 'sent',
    uiState: favorited ? { favorited: true } : undefined
  }
}

it('places the favorite action after copy and persists the selected state', async () => {
  const onUiStateChange = vi.fn()
  const screen = await render(
    <ChatMessageItem
      message={userMessage()}
      onUiStateChange={onUiStateChange}
      showTokenUsageDetails={false}
    />
  )

  const actionButtons = Array.from(
    screen.container.querySelectorAll<HTMLButtonElement>('.chat-message__actions > button')
  )
  expect(actionButtons.map((button) => button.getAttribute('aria-label'))).toEqual([
    'chat.copyMessage',
    'chat.favoriteMessage'
  ])

  const favoriteButton = screen.getByRole('button', { name: 'chat.favoriteMessage' })
  await expect.element(favoriteButton).toHaveAttribute('aria-pressed', 'false')
  await favoriteButton.click()

  expect(onUiStateChange).toHaveBeenCalledWith('user-1', { favorited: true })
})

it('renders a filled star and removes empty UI state when unfavorited', async () => {
  const onUiStateChange = vi.fn()
  const screen = await render(
    <ChatMessageItem
      message={userMessage(true)}
      onUiStateChange={onUiStateChange}
      showTokenUsageDetails={false}
    />
  )

  const favoriteButton = screen.getByRole('button', { name: 'chat.unfavoriteMessage' })
  await expect.element(favoriteButton).toHaveAttribute('aria-pressed', 'true')
  expect(favoriteButton.element().querySelector('svg')).toHaveAttribute('fill', 'currentColor')

  await favoriteButton.click()
  expect(onUiStateChange).toHaveBeenCalledWith('user-1', undefined)
})
