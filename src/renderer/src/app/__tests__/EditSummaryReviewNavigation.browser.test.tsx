import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatMessage } from '../../features/chat/chatTypes'
import { ChatMessageItem } from '../../features/chat/components/ChatMessageItem'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => key
  })
}))

vi.mock('../../features/storage/storageClient', () => ({
  loadAttachmentImage: vi.fn(),
  loadImageFile: vi.fn(),
  revealStoredProjectFile: vi.fn()
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {}
}))

describe('edit summary review navigation', () => {
  it('routes every diff card through the same workspace last-turn callback', async () => {
    const onReviewLastTurn = vi.fn()
    const screen = await render(
      <div>
        <ChatMessageItem
          message={assistantMessage('assistant-older', 'run-older', 'older.txt')}
          onReviewLastTurn={onReviewLastTurn}
          projectId="project-1"
          showTokenUsageDetails={false}
        />
        <ChatMessageItem
          message={assistantMessage('assistant-newer', 'run-newer', 'newer.txt')}
          onReviewLastTurn={onReviewLastTurn}
          projectId="project-1"
          showTokenUsageDetails={false}
        />
      </div>
    )

    const buttons = Array.from(
      screen.container.querySelectorAll<HTMLButtonElement>('.edit-summary-card__review')
    )
    expect(buttons).toHaveLength(2)
    buttons[0]?.click()
    buttons[1]?.click()

    expect(onReviewLastTurn.mock.calls).toEqual([[], []])
  })
})

function assistantMessage(id: string, runId: string, filePath: string): ChatMessage {
  return {
    id,
    role: 'assistant',
    content: 'done',
    createdAt: 1,
    status: 'sent',
    agentRun: {
      approvals: [],
      completedAt: 2,
      diffs: [],
      fileDrafts: [
        {
          additions: 1,
          byteCount: 4,
          chunkCount: 1,
          conversationId: 'conversation-1',
          createdAt: 1,
          deletions: 0,
          draftId: `${runId}-draft`,
          filePath,
          lineCount: 1,
          mode: 'create',
          nextChunkIndex: 1,
          projectId: 'project-1',
          statsFinal: true,
          status: 'applied',
          updatedAt: 2
        }
      ],
      runId,
      startedAt: 1,
      status: 'completed',
      timeline: [],
      toolCalls: [],
      toolDefinitions: [],
      toolResults: []
    }
  }
}
