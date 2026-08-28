import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { GitTurnDiffSummary } from '@mycopilot/protocol'
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
  it('renders the authoritative net line counts instead of accumulated draft counts', async () => {
    const screen = await render(
      <ChatMessageItem
        message={assistantMessage('assistant-1', 'run-1', 'src/edited.ts')}
        projectId="project-1"
        showTokenUsageDetails={false}
        turnDiffSummary={turnDiffSummary('assistant-1', 'src/edited.ts')}
      />
    )

    const card = screen.container.querySelector('.edit-summary-card')
    expect(card?.textContent).toContain('+35')
    expect(card?.textContent).toContain('-1')
  })

  it('routes every review button through the same workspace last-turn callback', async () => {
    const onReviewLastTurn = vi.fn()
    const screen = await render(
      <div>
        <ChatMessageItem
          message={assistantMessage('assistant-older', 'run-older', 'older.txt')}
          onReviewLastTurn={onReviewLastTurn}
          projectId="project-1"
          showTokenUsageDetails={false}
          turnDiffSummary={turnDiffSummary('assistant-older', 'older.txt')}
        />
        <ChatMessageItem
          message={assistantMessage('assistant-newer', 'run-newer', 'newer.txt')}
          onReviewLastTurn={onReviewLastTurn}
          projectId="project-1"
          showTokenUsageDetails={false}
          turnDiffSummary={turnDiffSummary('assistant-newer', 'newer.txt')}
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

  it('passes a clicked file path through the last-turn review callback', async () => {
    const onReviewLastTurn = vi.fn()
    const screen = await render(
      <ChatMessageItem
        message={assistantMessage('assistant-1', 'run-1', 'src/edited.ts')}
        onReviewLastTurn={onReviewLastTurn}
        projectId="project-1"
        showTokenUsageDetails={false}
        turnDiffSummary={turnDiffSummary('assistant-1', 'src/edited.ts')}
      />
    )

    const fileButton = screen.container.querySelector<HTMLButtonElement>(
      '.edit-summary-card__file-button'
    )
    if (!fileButton) throw new Error('Edited-file review button did not render')
    fileButton.click()

    expect(onReviewLastTurn).toHaveBeenCalledWith('src/edited.ts')
  })
})

function turnDiffSummary(assistantMessageId: string, filePath: string): GitTurnDiffSummary {
  return {
    assistantMessageId,
    files: [
      {
        path: filePath,
        stats: {
          additions: 35,
          deletions: 1
        },
        status: 'modified'
      }
    ],
    stats: {
      additions: 35,
      deletions: 1,
      fileCount: 1,
      lineCountsComplete: true
    },
    truncated: false
  }
}

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
      fileChangeProposals: [],
      fileChanges: [
        {
          schemaVersion: 1,
          additions: 1,
          byteCount: 4,
          conversationId: 'conversation-1',
          createdAt: 1,
          deletions: 0,
          transactionId: `${runId}-draft`,
          filePath,
          lineCount: 1,
          operation: 'create',
          updateStrategy: null,
          baseRevision: null,
          mutationCount: 1,
          nextMutationIndex: 1,
          summary: null,
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
