import { expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatConversation } from '../../features/chat/chatTypes'
import { useConversationAttention } from '../../features/chat/useConversationAttention'

vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

function Harness({ conversation }: { conversation: ChatConversation }) {
  return <output>{JSON.stringify(useConversationAttention([conversation]))}</output>
}

it('does not rescan the same message array for fallback attention on unrelated rerenders', async () => {
  let scans = 0
  const messages = new Proxy<ChatConversation['messages']>(
    Array.from({ length: 1000 }, (_, index) => ({
      id: `reply-${index}`,
      role: 'assistant',
      content: 'Completed history',
      createdAt: index,
      status: 'sent'
    })),
    {
      get(target, property, receiver) {
        if (property === Symbol.iterator) scans++
        return Reflect.get(target, property, receiver)
      }
    }
  )
  const conversation: ChatConversation = {
    id: 'chat',
    title: 'Chat',
    projectId: null,
    modelId: null,
    messages,
    createdAt: 1,
    updatedAt: 1
  }
  const screen = await render(<Harness conversation={conversation} />)
  expect(scans).toBe(1)
  for (let index = 0; index < 30; index++) {
    await screen.rerender(<Harness conversation={{ ...conversation, updatedAt: index + 2 }} />)
  }
  expect(scans).toBe(1)
})
