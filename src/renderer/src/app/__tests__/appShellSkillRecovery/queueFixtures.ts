import type { ChatQueuedMessage } from '../../../features/chat/chatTypes'
import { testState } from './state'
import { mockSuccessfulTurnStarts } from './turnFixtures'
import { emitAgentEvent } from './eventFixtures'
import { createComposerDraft } from './setup'

export function loadQueue(messages: ChatQueuedMessage[]) {
  testState.loadComposerDrafts.mockResolvedValueOnce({
    'conversation-a': createComposerDraft({
      modelId: 'model-1',
      projectId: 'project-a',
      queuedMessages: messages
    })
  })
  mockSuccessfulTurnStarts()
}

export function finishRun(
  runId: string,
  status: 'completed' | 'failed' | 'cancelled' = 'completed'
) {
  emitAgentEvent({
    type: 'done',
    runId,
    success: status === 'completed',
    status,
    content: status === 'completed' ? 'finished current run' : ''
  })
}
