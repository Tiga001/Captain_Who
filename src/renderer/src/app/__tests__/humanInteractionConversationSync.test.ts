import { expect, it, vi } from 'vitest'
import type { ChatConversation } from '../../features/chat/chatTypes'
vi.mock('../../host/hostClient', () => ({ hostClient: {} }))
import { isRecoverableGuidanceItem } from '../recoverableGuidance'
import { humanInteractionResponseDisplay } from '../../features/humanInteraction/humanInteractionState'
import {
  question,
  submitted
} from '../../features/humanInteraction/__tests__/humanInteractionFixtures'
import { mergeHumanInteractionConversation } from '../useHumanInteractionConversationSync'

const baseline: ChatConversation = {
  id: 'chat',
  title: 'Original',
  modelId: null,
  projectId: null,
  createdAt: 1,
  updatedAt: 1,
  messages: [{ id: 'old', role: 'assistant', content: 'Before', createdAt: 1 }]
}
it('attaches Host-created answer/assistant rows and preserves streaming and pending local messages received during a load', () => {
  const current: ChatConversation = {
    ...baseline,
    title: 'Renamed',
    messages: [
      { ...baseline.messages[0], content: 'New live text', uiState: { favorited: true } },
      { id: 'local', role: 'user', content: 'Composer input', status: 'pending', createdAt: 4 }
    ]
  }
  const stored: ChatConversation = {
    ...baseline,
    updatedAt: 3,
    messages: [
      ...baseline.messages,
      { id: 'answer', role: 'user', content: 'Host answer', createdAt: 2 },
      { id: 'new-run', role: 'assistant', content: 'Continued', createdAt: 3 }
    ]
  }
  const next = mergeHumanInteractionConversation(current, stored, baseline)
  expect(next.messages.map((message) => message.id)).toEqual(['old', 'answer', 'new-run', 'local'])
  expect(next.messages[0]).toBe(current.messages[0])
  expect(next.title).toBe('Renamed')
  expect(current.messages).toHaveLength(2)
})
it('refreshes unchanged waiting history from Host without changing metadata or local UI state', () => {
  const stored = {
    ...baseline,
    messages: [{ ...baseline.messages[0], content: 'Authoritative result' }]
  }
  expect(mergeHumanInteractionConversation(baseline, stored, baseline).messages[0].content).toBe(
    'Authoritative result'
  )
})

it('keeps Host-owned answered guidance out of Composer recovery without swallowing ordinary JSON input', () => {
  const item = {
    id: 'answer',
    type: 'user_guidance' as const,
    guidanceId: 'g1',
    clientMessageId: 'human-answer-g1',
    content: JSON.stringify(humanInteractionResponseDisplay(submitted(question()))),
    attachments: [],
    createdAt: 1,
    status: 'rejected' as const,
    recoverable: true,
    rejectionCode: 'run_interrupted'
  }
  expect(isRecoverableGuidanceItem(item)).toBe(false)
  expect(isRecoverableGuidanceItem({ ...item, clientMessageId: 'ordinary-guidance-id' })).toBe(true)
})
