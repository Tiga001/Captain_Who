import type { AgentProposedAction } from '@mycopilot/protocol'
import { useRef } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatConversation } from '../../features/chat/chatTypes'

const service = vi.hoisted(() => ({
  approve: vi.fn(),
  cancel: vi.fn(),
  reject: vi.fn()
}))

vi.mock('../../features/agent/agentClient', () => ({
  approveAgentAction: service.approve,
  cancelAgentAction: service.cancel,
  rejectAgentAction: service.reject
}))

const { useAgentActionDecisionHandlers } =
  await import('../../features/agentRun/useAgentActionDecisionHandlers')

const ERROR_CANARY = 'MCP_HOST_ERROR_CANARY_MUST_NOT_REACH_CONSOLE'

function mcpAction(): Extract<AgentProposedAction, { type: 'mcp_tool_call' }> {
  return {
    type: 'mcp_tool_call',
    approval: {
      identity: {
        actionId: '11111111-1111-4111-8111-111111111111',
        invocationId: '22222222-2222-4222-8222-222222222222',
        runId: 'run-mcp-console-safety',
        callId: `tc1_${'a'.repeat(43)}`,
        provenance: {
          serverId: '33333333-3333-4333-8333-333333333333',
          scope: { type: 'user' },
          rawToolName: 'echo_text',
          modelToolName: 'provider_safe_echo',
          configEpoch: '44444444-4444-4444-8444-444444444444',
          registryRevision: 1,
          configDigest: 'a'.repeat(64),
          catalogGeneration: 1,
          catalogDigest: 'b'.repeat(64),
          catalogSchemaDigest: 'c'.repeat(64),
          schemaDigest: 'd'.repeat(64),
          schemaNormalizerVersion: 1
        }
      },
      call: {
        id: `tc1_${'a'.repeat(43)}`,
        tool: 'provider_safe_echo',
        args: {},
        approvalStatus: 'required',
        reason: null
      },
      summary: {
        serverId: '33333333-3333-4333-8333-333333333333',
        serverDisplayName: 'Owned fixture',
        scope: { type: 'user' },
        rawToolName: 'echo_text',
        modelToolName: 'provider_safe_echo',
        displayReason: null,
        arguments: {
          encodedBytes: 2,
          topLevelPropertyCount: 0,
          stringValueCount: 0,
          numberValueCount: 0,
          booleanValueCount: 0,
          nullValueCount: 0,
          objectValueCount: 1,
          arrayValueCount: 0,
          maxDepth: 1,
          truncated: false
        },
        risk: 'unknown',
        external: true
      },
      approvalMode: 'prompt',
      payloadPersistence: 'process_only',
      createdAt: 1,
      expiresAt: 15 * 60 * 1000
    }
  }
}

function fileChangeAction(): Extract<AgentProposedAction, { type: 'file_change' }> {
  return {
    type: 'file_change',
    fileChange: {
      schemaVersion: 1,
      id: 'file-change-console-safety',
      transactionId: 'file-change-transaction-console-safety',
      operation: 'create',
      updateStrategy: null,
      filePath: 'safe.txt',
      inlineDiff: { patch: '@@ -0,0 +1 @@\n+safe\n', truncated: false },
      baseRevision: null,
      summary: 'Create a safe fixture.',
      additions: 1,
      deletions: 0,
      lineCount: 1,
      byteCount: 5,
      approvalStatus: 'required'
    }
  }
}

function Harness({ action }: { action: AgentProposedAction }) {
  const conversationsRef = useRef<ChatConversation[]>([
    {
      id: 'conversation-mcp-console-safety',
      projectId: null,
      modelId: null,
      title: 'MCP safety',
      createdAt: 1,
      updatedAt: 1,
      messages: [
        {
          id: 'assistant-mcp-console-safety',
          role: 'assistant',
          content: '',
          createdAt: 1,
          status: 'pending',
          agentRun: {
            runId: 'run-mcp-console-safety',
            status: 'waiting_for_approval',
            toolDefinitions: [],
            toolCalls: [],
            toolResults: [],
            approvals: [action],
            fileChangeProposals: [],
            timeline: []
          }
        }
      ]
    }
  ])
  const { handleApproveAgentAction } = useAgentActionDecisionHandlers({
    activeConversationId: 'conversation-mcp-console-safety',
    conversationsRef,
    updateAssistantMessage: vi.fn()
  })

  return (
    <button
      onClick={() =>
        handleApproveAgentAction('assistant-mcp-console-safety', action, 'singleAction')
      }
      type="button"
    >
      approve MCP
    </button>
  )
}

afterEach(() => {
  vi.restoreAllMocks()
  service.approve.mockReset()
  service.cancel.mockReset()
  service.reject.mockReset()
})

describe('protected pending-action logging safety', () => {
  it('does not log Host Error objects, causes, stacks, or messages', async () => {
    const error = new Error(ERROR_CANARY, { cause: { rawArguments: ERROR_CANARY } })
    service.approve.mockRejectedValue(error)
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const screen = await render(<Harness action={mcpAction()} />)

    await screen.getByRole('button', { name: 'approve MCP' }).click()
    await expect.poll(() => service.approve.mock.calls.length).toBe(1)
    expect(service.approve).toHaveBeenCalledWith(
      'run-mcp-console-safety',
      '11111111-1111-4111-8111-111111111111',
      'singleAction'
    )
    await expect.poll(() => consoleError.mock.calls.length).toBe(1)

    expect(consoleError).toHaveBeenCalledWith('Failed to approve protected Agent action')
    const logged = consoleError.mock.calls
      .flat()
      .map((value) => (value instanceof Error ? `${value.message}\n${value.stack}` : String(value)))
      .join('\n')
    expect(logged).not.toContain(ERROR_CANARY)
  })

  it('does not log FileChange decision Error objects, causes, stacks, or storage messages', async () => {
    const error = new Error(`SQLite no such table: ${ERROR_CANARY}`, {
      cause: { rustVariant: `Database(${ERROR_CANARY})` }
    })
    service.approve.mockRejectedValue(error)
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const action = fileChangeAction()
    const screen = await render(<Harness action={action} />)

    await screen.getByRole('button', { name: 'approve MCP' }).click()
    await expect.poll(() => service.approve.mock.calls.length).toBe(1)
    expect(service.approve).toHaveBeenCalledWith(
      'run-mcp-console-safety',
      'file-change-console-safety',
      'singleAction'
    )
    await expect.poll(() => consoleError.mock.calls.length).toBe(1)

    expect(consoleError).toHaveBeenCalledWith('Failed to approve protected Agent action')
    const logged = consoleError.mock.calls
      .flat()
      .map((value) => (value instanceof Error ? `${value.message}\n${value.stack}` : String(value)))
      .join('\n')
    expect(logged).not.toContain('SQLite')
    expect(logged).not.toContain('Database(')
    expect(logged).not.toContain(ERROR_CANARY)
  })
})
