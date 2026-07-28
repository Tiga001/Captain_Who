import { expect, it, vi } from 'vitest'
import type { ChatMessage } from '../chatTypes'

const storage = vi.hoisted(() => ({
  saveChatMessageState: vi.fn()
}))

vi.mock('../../../host/hostClient', () => ({
  hostClient: { storage }
}))

const { saveChatMessageState } = await import('../../storage/storageClient')

it('keeps live command output transient while persisting the final tool result', async () => {
  const message: ChatMessage = {
    id: 'assistant-command',
    role: 'assistant',
    content: 'Command complete.',
    createdAt: 1,
    status: 'sent',
    agentRun: {
      runId: 'run-command',
      status: 'completed',
      toolDefinitions: [],
      toolCalls: [
        {
          id: 'command-call',
          tool: 'run_command',
          args: { command: 'printf done' },
          approvalStatus: 'not_required'
        }
      ],
      toolResults: [
        {
          callId: 'command-call',
          tool: 'run_command',
          ok: true,
          result: {
            command: 'printf done',
            stdout: 'done',
            stderr: '',
            exitCode: 0
          }
        }
      ],
      approvals: [],
      diffs: [],
      commandOutputPreviews: {
        'command-call': {
          callId: 'command-call',
          chunks: [{ sequence: 1, stream: 'stdout', output: 'done' }]
        }
      },
      timeline: []
    }
  }

  storage.saveChatMessageState.mockResolvedValueOnce(undefined)
  await saveChatMessageState('conversation-command', message)

  const storedMessage = storage.saveChatMessageState.mock.calls[0]?.[0]?.message
  const storedRun = JSON.parse(storedMessage.agentRunJson) as Record<string, unknown>
  expect(storedRun.commandOutputPreviews).toBeUndefined()
  expect(storedRun.toolResults).toEqual(message.agentRun?.toolResults)
})
