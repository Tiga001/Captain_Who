import { expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { ConversationSurface } from '../ConversationSurface'
import type { ChatConversation } from '../chatTypes'
import { EditSummaryCard } from '../components/EditSummaryCard'

const mocks = vi.hoisted(() => ({
  getAgentFileWriteDiff: vi.fn(),
  getTurnDiffSummaries: vi.fn(),
  loadAttachmentImage: vi.fn(),
  readAgentFileDraft: vi.fn()
}))

const translations: Record<string, string> = {
  'agent.processed': 'Processed {duration}',
  'agent.read.file.completed': 'Read file',
  'agent.read.item': '{fileName}',
  'chat.copyMessage': 'Copy message',
  'chat.fromParentAgent': 'From parent agent',
  'chat.fromCollaboratingAgent': 'From collaborating agent',
  'chat.historicalContext': 'Inherited context snapshot',
  'chat.messageActions': 'Message actions',
  'chat.usage': 'Usage',
  'chat.usageTitle': 'Usage details'
}

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'en-US',
    t: (key: string) => translations[key] ?? key
  })
}))

vi.mock('../../storage/storageClient', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../storage/storageClient')>()),
  loadAttachmentImage: mocks.loadAttachmentImage,
  loadImageFile: vi.fn(),
  revealStoredProjectFile: vi.fn()
}))

vi.mock('../../gitReview/gitReviewClient', () => ({
  getGitTurnDiffSummaries: mocks.getTurnDiffSummaries
}))

vi.mock('../../agent/agentClient', () => ({
  getAgentFileWriteDiff: mocks.getAgentFileWriteDiff,
  readAgentFileDraft: mocks.readAgentFileDraft
}))

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: vi.fn() })
}))

const readCall: AgentToolCall = {
  id: 'read-call',
  tool: 'read_file',
  args: { path: 'notes.md' },
  approvalStatus: 'not_required',
  reason: null
}
const readResult: AgentToolResult = {
  callId: readCall.id,
  tool: readCall.tool,
  ok: true,
  result: { path: 'notes.md' }
}
const skillId = 'bundled:application:spreadsheets'
const skillResourceUri = `skill://package/${encodeURIComponent(skillId)}/revision-1/references/workflows.md`
const skillCall: AgentToolCall = {
  id: 'skill-call',
  tool: 'skills_read_resource',
  args: { uri: skillResourceUri },
  approvalStatus: 'not_required',
  reason: null
}
const commandCall: AgentToolCall = {
  id: 'command-call',
  tool: 'run_command',
  args: { command: 'render-report', reason: 'Build the child report.' },
  approvalStatus: 'approved',
  reason: null
}
const writeCall: AgentToolCall = {
  id: 'write-call',
  tool: 'write_file',
  args: { filePath: 'src/report.ts', mode: 'create' },
  approvalStatus: 'approved',
  reason: null
}

function observerConversation(id = 'child-conversation'): ChatConversation {
  return {
    id,
    projectId: 'project-1',
    modelId: 'child-model',
    title: 'Child observer',
    createdAt: 1,
    updatedAt: 4,
    messages: [
      {
        id: 'parent-task',
        role: 'user',
        content: '**Investigate** the issue.',
        createdAt: 2,
        status: 'sent',
        inputOrigin: {
          kind: 'agent',
          senderAgentId: 'parent-agent',
          sourceAgentMessageId: 'mailbox-task',
          snapshotSourceConversationId: null,
          snapshotSourceMessageId: null
        },
        attachments: [
          {
            id: 'observer-image',
            kind: 'image',
            name: 'observer.png',
            mimeType: 'image/png',
            sizeBytes: 4,
            encoding: 'base64',
            data: 'AAAA',
            previewData: 'AAAA',
            previewMimeType: 'image/png'
          }
        ]
      },
      {
        id: 'child-answer',
        role: 'assistant',
        content: 'The child completed its analysis.',
        createdAt: 3,
        status: 'sent',
        agentRun: {
          runId: 'child-run',
          status: 'completed',
          startedAt: 2,
          completedAt: 4,
          toolDefinitions: [],
          toolCalls: [readCall, skillCall, commandCall, writeCall],
          toolResults: [
            readResult,
            {
              callId: skillCall.id,
              tool: skillCall.tool,
              ok: true,
              result: { uri: skillResourceUri }
            },
            {
              callId: commandCall.id,
              tool: commandCall.tool,
              ok: true,
              result: { command: 'render-report', exitCode: 0, stderr: '', stdout: 'done' }
            },
            {
              callId: writeCall.id,
              tool: writeCall.tool,
              ok: true,
              result: { draftId: 'draft-child', status: 'applied' }
            }
          ],
          approvals: [],
          diffs: [],
          activatedSkills: [
            {
              id: skillId,
              name: 'Spreadsheets',
              revision: 'revision-1',
              source: { kind: 'bundled', id: 'application:spreadsheets' }
            }
          ],
          commandSessions: {
            [commandCall.id]: {
              callId: commandCall.id,
              status: 'exited',
              startedAt: 2,
              endedAt: 3,
              exitCode: 0,
              latestSequence: 1,
              outputTruncated: false,
              outputs: [
                {
                  name: 'reports/child-report.pdf',
                  kind: 'document',
                  readPath: `artifact://sha256/${'a'.repeat(64)}`,
                  mimeType: 'application/pdf',
                  sizeBytes: 4096,
                  sha256: 'a'.repeat(64)
                }
              ]
            }
          },
          fileDrafts: [
            {
              draftId: 'draft-child',
              conversationId: 'child-conversation',
              projectId: 'project-1',
              filePath: 'src/report.ts',
              mode: 'create',
              status: 'applied',
              additions: 1,
              deletions: 0,
              lineCount: 1,
              byteCount: 20,
              chunkCount: 1,
              nextChunkIndex: 1,
              statsFinal: true,
              createdAt: 2,
              updatedAt: 3
            }
          ],
          mcpInvocations: [
            {
              actionId: '11111111-1111-4111-8111-111111111111',
              invocationId: '22222222-2222-4222-8222-222222222222',
              callId: 'mcp-call',
              serverId: '33333333-3333-4333-8333-333333333333',
              serverDisplayName: 'Test MCP',
              rawToolName: 'inspect',
              modelToolName: 'mcp__test__inspect',
              displayReason: 'Inspect the fixture safely.',
              external: true,
              state: 'completed',
              dispatchCertainty: 'response_received',
              outcome: 'succeeded',
              isError: false,
              durationMs: 5,
              outputTruncated: false
            }
          ],
          timeline: [
            { id: 'read-timeline', type: 'tool_call', callId: readCall.id },
            { id: 'skill-timeline', type: 'tool_call', callId: skillCall.id },
            { id: 'command-timeline', type: 'tool_call', callId: commandCall.id },
            { id: 'write-timeline', type: 'tool_call', callId: writeCall.id },
            {
              id: 'mcp-timeline',
              type: 'mcp_tool_call',
              invocationId: '22222222-2222-4222-8222-222222222222'
            }
          ],
          usage: { inputTokens: 11, outputTokens: 7, totalTokens: 18 }
        }
      }
    ]
  }
}

it('reuses the chat Timeline in observer mode while exposing no child write controls', async () => {
  mocks.getAgentFileWriteDiff.mockResolvedValue({
    draftId: 'draft-child',
    patch: '+export const child = true',
    offset: 0,
    truncated: false
  })
  const screen = await render(
    <ConversationSurface
      agentLabelsById={{ 'parent-agent': 'Root planner' }}
      conversation={observerConversation()}
      mode="observer"
      parentAgentId="parent-agent"
      rootConversationId="root-conversation"
      showTokenUsageDetails
    />
  )

  await expect.element(screen.getByText('Investigate')).toBeVisible()
  await expect.element(screen.getByText('From parent agent')).toBeVisible()
  await expect.element(screen.getByText('Root planner')).toBeVisible()
  await expect.element(screen.getByText('The child completed its analysis.')).toBeVisible()
  expect(
    screen.container.querySelector('.chat-message__usage button')?.getAttribute('aria-label')
  ).toBe('Usage')

  expect(screen.container.querySelector('.chat-conversation-page__composer')).toBeNull()
  expect(screen.container.querySelector('textarea')).toBeNull()
  expect(screen.container.querySelector('.agent-approval-dialog')).toBeNull()
  expect(screen.container.querySelector('[aria-label="chat.editMessage"]')).toBeNull()
  expect(screen.container.querySelector('[aria-label="chat.continueInNewTask"]')).toBeNull()
  expect(screen.container.querySelector('[aria-label="chat.favoriteMessage"]')).toBeNull()
  expect(mocks.getTurnDiffSummaries).not.toHaveBeenCalled()

  await screen.container
    .querySelector<HTMLButtonElement>('[data-chat-attachment-id="observer-image"]')
    ?.click()
  expect(mocks.loadAttachmentImage).not.toHaveBeenCalled()

  const timelineToggle = screen.container.querySelector<HTMLButtonElement>(
    '.agent-run__elapsed-button'
  )
  expect(timelineToggle).not.toBeNull()
  expect(screen.container.textContent).not.toContain('notes.md')
  await timelineToggle?.click()
  expect(screen.container.textContent).toContain('notes.md')
  expect(screen.container.querySelector('.agent-activity--skill')).not.toBeNull()
  expect(screen.container.querySelector('.agent-activity--skill-resource')).not.toBeNull()
  expect(screen.container.querySelector('.agent-activity--run-command')).not.toBeNull()
  expect(screen.container.querySelector('.mcp-tool-activity')).not.toBeNull()
  expect(screen.container.querySelector('.office-artifact-card')).not.toBeNull()
  await screen.container.querySelector<HTMLButtonElement>('.file-write-activity__toggle')?.click()
  await expect
    .element(screen.getByText('+export const child = true', { exact: true }))
    .toBeVisible()
  expect(mocks.getAgentFileWriteDiff).toHaveBeenCalledWith(
    'draft-child',
    0,
    50_000,
    'root-conversation'
  )
})

it('renders a persisted edit summary read-only without review or undo actions', async () => {
  const onReview = vi.fn()
  const screen = await render(
    <EditSummaryCard
      onReview={onReview}
      readOnly
      summary={{
        assistantMessageId: 'child-answer',
        files: [
          {
            path: 'src/child-edit.ts',
            stats: { additions: 3, deletions: 1 },
            status: 'modified'
          }
        ],
        stats: { additions: 3, deletions: 1, fileCount: 1, lineCountsComplete: true },
        truncated: false
      }}
    />
  )

  expect(screen.container.textContent).toContain('src/child-edit.ts')
  expect(screen.container.querySelector('.edit-summary-card__actions')).toBeNull()
  expect(screen.container.querySelector('.edit-summary-card__file-button')).toBeNull()
  expect(onReview).not.toHaveBeenCalled()
})

it('resets observer-local Timeline disclosure state when switching conversations', async () => {
  const screen = await render(
    <ConversationSurface
      conversation={observerConversation('child-a')}
      mode="observer"
      rootConversationId="root-conversation"
      showTokenUsageDetails={false}
    />
  )
  await screen.container.querySelector<HTMLButtonElement>('.agent-run__elapsed-button')?.click()
  expect(screen.container.textContent).toContain('notes.md')

  await screen.rerender(
    <ConversationSurface
      conversation={observerConversation('child-b')}
      mode="observer"
      rootConversationId="root-conversation"
      showTokenUsageDetails={false}
    />
  )
  await new Promise((resolve) => window.setTimeout(resolve, 0))
  expect(
    screen.container.querySelector('.conversation-surface')?.getAttribute('data-conversation-id')
  ).toBe('child-b')
  expect(screen.container.textContent).not.toContain('notes.md')
})
