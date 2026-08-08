import { expect, it, vi } from 'vitest'
import type { ChatMessage } from '../chatTypes'

const storage = vi.hoisted(() => ({
  loadConversation: vi.fn(),
  saveChatMessageState: vi.fn()
}))

vi.mock('../../../host/hostClient', () => ({
  hostClient: { storage }
}))

const { loadConversation, saveChatMessageState } = await import('../../storage/storageClient')

function storedMcpApprovalAction(callId: string) {
  const serverId = 'ce18d23c-e74f-4e89-8695-ce1e7c60ec92'
  return {
    type: 'mcp_tool_call',
    approval: {
      identity: {
        actionId: '94c2f39c-ddaa-49bb-a3ef-8756053d68c8',
        invocationId: 'a8a6102c-8ad6-45d5-bb0d-3e4f0ad2a30f',
        runId: 'run-mcp-approval-only',
        callId,
        provenance: {
          serverId,
          scope: { type: 'user' },
          rawToolName: 'move_file',
          modelToolName: 'mcp__filesystem_test__move_file',
          configEpoch: '41818332-0842-4d2e-808f-175b70eb4628',
          registryRevision: 7,
          configDigest: 'a'.repeat(64),
          catalogGeneration: 2,
          catalogDigest: 'b'.repeat(64),
          catalogSchemaDigest: 'c'.repeat(64),
          schemaDigest: 'd'.repeat(64),
          schemaNormalizerVersion: 1
        }
      },
      call: {
        id: callId,
        tool: 'mcp__filesystem_test__move_file',
        args: {},
        approvalStatus: 'required'
      },
      summary: {
        serverId,
        serverDisplayName: 'Filesystem Test',
        scope: { type: 'user' },
        rawToolName: 'move_file',
        modelToolName: 'mcp__filesystem_test__move_file',
        arguments: {
          encodedBytes: 24,
          topLevelPropertyCount: 1,
          stringValueCount: 1,
          numberValueCount: 0,
          booleanValueCount: 0,
          nullValueCount: 0,
          objectValueCount: 1,
          arrayValueCount: 0,
          maxDepth: 2,
          truncated: false
        },
        risk: 'side_effects_possible',
        external: true
      },
      approvalMode: 'prompt',
      payloadPersistence: 'process_only',
      createdAt: 1_753_843_200_000,
      expiresAt: 1_753_844_100_000
    }
  }
}

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
      commandSessions: {
        'command-call': {
          callId: 'command-call',
          sessionId: 'cmd_1234567890abcdef1234567890abcdef',
          status: 'running',
          startedAt: 1,
          latestSequence: 1,
          outputTruncated: false
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
  expect(storedRun.commandSessions).toBeUndefined()
  expect(storedRun.toolResults).toEqual(message.agentRun?.toolResults)
})

it('discards legacy persisted managed command projections during reload', async () => {
  storage.loadConversation.mockResolvedValueOnce({
    id: 'conversation-command-reload',
    projectId: null,
    modelId: 'model-1',
    title: 'Managed command reload',
    messages: [
      {
        id: 'assistant-command-reload',
        role: 'assistant',
        content: 'The process was handed off.',
        createdAt: 1,
        status: 'sent',
        attachments: [],
        agentRunJson: JSON.stringify({
          runId: 'run-command-reload',
          status: 'completed',
          completedAt: 2,
          toolDefinitions: [],
          toolCalls: [
            {
              id: 'command-call',
              tool: 'run_command',
              args: { command: 'long-running' },
              approvalStatus: 'approved'
            }
          ],
          toolResults: [],
          approvals: [],
          diffs: [],
          timeline: [{ id: 'tool-call-command-call', type: 'tool_call', callId: 'command-call' }],
          commandSessions: {
            'command-call': {
              callId: 'command-call',
              sessionId: 'cmd_1234567890abcdef1234567890abcdef',
              status: 'running',
              startedAt: 1,
              latestSequence: 1,
              outputTruncated: false
            }
          },
          commandOutputPreviews: {
            'command-call': {
              callId: 'command-call',
              chunks: [{ sequence: 1, stream: 'stdout', output: 'stale output' }]
            }
          }
        }),
        uiStateJson: null
      }
    ],
    createdAt: 1,
    updatedAt: 2,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  })

  const restored = await loadConversation('conversation-command-reload')

  expect(restored?.messages[0].agentRun?.commandSessions).toBeUndefined()
  expect(restored?.messages[0].agentRun?.commandOutputPreviews).toBeUndefined()
})

it('persists and reloads only immutable command terminal metadata', async () => {
  const message: ChatMessage = {
    id: 'assistant-command-terminal',
    role: 'assistant',
    content: 'The application exited successfully.',
    createdAt: 1,
    status: 'sent',
    agentRun: {
      runId: 'run-command-terminal',
      status: 'completed',
      completedAt: 20,
      toolDefinitions: [],
      toolCalls: [
        {
          id: 'command-call',
          tool: 'run_command',
          args: { command: 'long-running' },
          approvalStatus: 'approved'
        }
      ],
      toolResults: [
        {
          callId: 'command-call',
          tool: 'run_command',
          ok: true,
          result: {
            status: 'running',
            sessionId: 'cmd_1234567890abcdef1234567890abcdef',
            output: 'ready\n',
            startedAt: 2,
            latestSequence: 1
          }
        }
      ],
      approvals: [],
      diffs: [],
      commandOutputPreviews: {
        'command-call': {
          callId: 'command-call',
          chunks: [{ sequence: 1, stream: 'stdout', output: 'ready\n' }]
        }
      },
      commandSessions: {
        'command-call': {
          callId: 'command-call',
          sessionId: 'cmd_1234567890abcdef1234567890abcdef',
          status: 'exited',
          startedAt: 2,
          endedAt: 12,
          exitCode: 0,
          latestSequence: 1,
          outputTruncated: false
        }
      },
      timeline: [{ id: 'tool-call-command-call', type: 'tool_call', callId: 'command-call' }]
    }
  }

  storage.saveChatMessageState.mockResolvedValueOnce(undefined)
  await saveChatMessageState('conversation-command-terminal', message)
  const storedMessage = storage.saveChatMessageState.mock.calls.at(-1)?.[0]?.message
  const storedRun = JSON.parse(storedMessage.agentRunJson) as Record<string, unknown>
  expect(storedRun.commandOutputPreviews).toBeUndefined()
  expect(storedRun.commandSessions).toEqual({
    'command-call': {
      callId: 'command-call',
      status: 'exited',
      startedAt: 2,
      endedAt: 12,
      exitCode: 0,
      latestSequence: 1,
      outputTruncated: false
    }
  })

  storage.loadConversation.mockResolvedValueOnce({
    id: 'conversation-command-terminal',
    projectId: null,
    modelId: 'model-1',
    title: 'Managed command terminal history',
    messages: [
      {
        id: message.id,
        role: message.role,
        content: message.content,
        createdAt: message.createdAt,
        status: message.status,
        attachments: [],
        agentRunJson: storedMessage.agentRunJson,
        uiStateJson: null
      }
    ],
    createdAt: 1,
    updatedAt: 20,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  })

  const restored = await loadConversation('conversation-command-terminal')
  expect(restored?.messages[0].agentRun?.commandSessions).toEqual(storedRun.commandSessions)
  expect(restored?.messages[0].agentRun?.commandOutputPreviews).toBeUndefined()
})

it('persists settled Skill installation activity across a conversation reload', async () => {
  const skillInstallation = {
    action: {
      schemaVersion: 1,
      id: 'skill-install-action-1',
      installRef: `skill_install_${'a'.repeat(32)}`,
      approvalStatus: 'approved' as const,
      expiresAt: 1_753_844_100_000,
      preview: {
        name: 'example-skill',
        description: 'Example third-party Skill.',
        sourceSummary: {
          kind: 'githubRepository',
          repository: 'example/skills',
          resolvedRevision: 'b'.repeat(40)
        },
        resolvedRevision: 'b'.repeat(40),
        fileCount: 4,
        totalBytes: 2_048,
        resourceSummary: {
          total: 3,
          references: 1,
          assets: 1,
          scripts: 1,
          bytes: 1_024
        },
        containsScripts: true,
        warnings: [
          {
            code: 'containsScripts',
            message: 'The package contains executable scripts.',
            requiresAcknowledgement: true
          }
        ],
        compatibility: 'compatibleWithWarnings',
        operation: 'install',
        impact: 'addManagedSkill'
      }
    },
    status: 'installed' as const
  }
  const message: ChatMessage = {
    id: 'assistant-skill-installation',
    role: 'assistant',
    content: 'The Skill is installed and available from the next run.',
    createdAt: 1,
    status: 'sent',
    agentRun: {
      runId: 'run-skill-installation',
      status: 'completed',
      completedAt: 10,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      skillInstallations: [skillInstallation],
      diffs: [],
      timeline: []
    }
  }

  storage.saveChatMessageState.mockResolvedValueOnce(undefined)
  await saveChatMessageState('conversation-skill-installation', message)
  const storedMessage = storage.saveChatMessageState.mock.calls.at(-1)?.[0]?.message

  storage.loadConversation.mockResolvedValueOnce({
    id: 'conversation-skill-installation',
    projectId: null,
    modelId: 'model-1',
    title: 'Skill installation',
    messages: [
      {
        id: message.id,
        role: message.role,
        content: message.content,
        createdAt: message.createdAt,
        status: message.status,
        attachments: [],
        agentRunJson: storedMessage.agentRunJson,
        uiStateJson: null
      }
    ],
    createdAt: 1,
    updatedAt: 10,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  })

  const restored = await loadConversation('conversation-skill-installation')
  expect(restored?.messages[0].agentRun?.skillInstallations).toEqual([skillInstallation])
})

it('fails closed when a restored terminal run still contains a running MCP invocation', async () => {
  const actionId = '94c2f39c-ddaa-49bb-a3ef-8756053d68c8'
  const invocationId = 'a8a6102c-8ad6-45d5-bb0d-3e4f0ad2a30f'
  const serverId = 'ce18d23c-e74f-4e89-8695-ce1e7c60ec92'
  const callId = `tc1_${'a'.repeat(43)}`
  const canary = 'MCP_STORAGE_CANARY_DO_NOT_RETAIN'
  storage.loadConversation.mockResolvedValueOnce({
    id: 'conversation-mcp-recovery',
    projectId: null,
    modelId: 'model-1',
    title: 'MCP recovery',
    messages: [
      {
        id: 'assistant-mcp-recovery',
        role: 'assistant',
        content: 'partial',
        createdAt: 1,
        status: 'error',
        attachments: [],
        agentRunJson: JSON.stringify({
          runId: 'run-mcp-recovery',
          status: 'failed',
          completedAt: 10,
          toolDefinitions: [],
          toolCalls: [
            {
              id: callId,
              tool: 'mcp__filesystem_test__move_file',
              args: { rawArguments: canary },
              approvalStatus: 'approved'
            }
          ],
          toolResults: [
            {
              callId,
              tool: 'mcp__filesystem_test__move_file',
              ok: true,
              result: { rawResult: canary },
              error: canary
            }
          ],
          approvals: [{ type: 'mcp_tool_call', rawArguments: canary }],
          diffs: [],
          timeline: [
            {
              id: `mcp-invocation-${invocationId}`,
              type: 'mcp_tool_call',
              invocationId,
              payloadRef: canary
            },
            {
              id: `tool-call-${callId}`,
              type: 'tool_call',
              callId
            }
          ],
          mcpInvocations: [
            {
              actionId,
              invocationId,
              callId,
              serverId,
              serverDisplayName: 'Filesystem Test',
              rawToolName: 'move_file',
              modelToolName: 'mcp__filesystem_test__move_file',
              displayReason: 'Move the approved fixture file',
              external: true,
              state: 'running',
              dispatchCertainty: 'possibly_dispatched',
              outputTruncated: false,
              rawArguments: canary,
              rawResult: canary,
              structuredContent: canary,
              stderr: canary,
              payloadRef: canary,
              ciphertext: canary
            }
          ]
        }),
        uiStateJson: null
      }
    ],
    createdAt: 1,
    updatedAt: 10,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  })

  const restored = await loadConversation('conversation-mcp-recovery')
  expect(restored?.messages[0].agentRun?.mcpInvocations?.[0]).toMatchObject({
    state: 'outcome_unknown',
    outcome: 'outcome_unknown',
    dispatchCertainty: 'possibly_dispatched',
    errorCode: 'mcp.tool_outcome_unknown',
    displayReason: 'Move the approved fixture file'
  })
  const restoredRun = restored?.messages[0].agentRun
  expect(restoredRun?.toolCalls).toEqual([])
  expect(restoredRun?.toolResults).toEqual([])
  expect(restoredRun?.approvals).toEqual([])
  expect(JSON.stringify(restoredRun)).not.toContain(canary)
  expect(Object.keys(restoredRun?.mcpInvocations?.[0] ?? {})).not.toContain('rawArguments')
})

it('normalizes a minimal terminal recovery record before settling activities', async () => {
  storage.loadConversation.mockResolvedValueOnce({
    id: 'conversation-minimal-recovery',
    projectId: null,
    modelId: 'model-1',
    title: 'Minimal recovery',
    messages: [
      {
        id: 'assistant-minimal-recovery',
        role: 'assistant',
        content: '',
        createdAt: 1,
        status: 'error',
        attachments: [],
        agentRunJson: JSON.stringify({
          runId: 'run-minimal-recovery',
          status: 'failed'
        }),
        uiStateJson: null
      }
    ],
    createdAt: 1,
    updatedAt: 2,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  })

  const restored = await loadConversation('conversation-minimal-recovery')
  expect(restored?.messages[0].agentRun).toMatchObject({
    runId: 'run-minimal-recovery',
    status: 'failed',
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    diffs: [],
    timeline: [],
    mcpInvocations: []
  })
})

it('replaces restored generic MCP trace anchors in place without moving calls above narration', async () => {
  const serverId = 'ce18d23c-e74f-4e89-8695-ce1e7c60ec92'
  const firstInvocationId = 'a8a6102c-8ad6-45d5-bb0d-3e4f0ad2a30f'
  const secondInvocationId = '513520d2-d01c-4a98-aeed-4d106df58ce2'
  const firstCallId = `tc1_${'d'.repeat(43)}`
  const secondCallId = `tc1_${'e'.repeat(43)}`
  const invocation = ({
    actionId,
    invocationId,
    callId,
    rawToolName
  }: {
    actionId: string
    invocationId: string
    callId: string
    rawToolName: string
  }) => ({
    actionId,
    invocationId,
    callId,
    serverId,
    serverDisplayName: 'Filesystem Test',
    rawToolName,
    modelToolName: `mcp__filesystem_test__${rawToolName}`,
    displayReason: `Use ${rawToolName}`,
    external: true,
    state: 'completed',
    dispatchCertainty: 'response_received',
    outcome: 'succeeded',
    isError: false,
    durationMs: 5,
    outputTruncated: false
  })

  storage.loadConversation.mockResolvedValueOnce({
    id: 'conversation-mcp-trace-order',
    projectId: null,
    modelId: 'model-1',
    title: 'MCP trace order',
    messages: [
      {
        id: 'assistant-mcp-trace-order',
        role: 'assistant',
        content: '第一段。第二段。第三段。',
        createdAt: 1,
        status: 'sent',
        attachments: [],
        agentRunJson: JSON.stringify({
          runId: 'run-mcp-trace-order',
          status: 'completed',
          completedAt: 10,
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          diffs: [],
          timeline: [
            { id: 'message-before-first', type: 'message', content: '第一段。' },
            { id: `tool-call-${firstCallId}`, type: 'tool_call', callId: firstCallId },
            { id: 'message-between-calls', type: 'message', content: '第二段。' },
            { id: `tool-call-${secondCallId}`, type: 'tool_call', callId: secondCallId },
            { id: 'message-after-second', type: 'message', content: '第三段。' }
          ],
          mcpInvocations: [
            invocation({
              actionId: '94c2f39c-ddaa-49bb-a3ef-8756053d68c8',
              invocationId: firstInvocationId,
              callId: firstCallId,
              rawToolName: 'list_allowed_directories'
            }),
            invocation({
              actionId: 'ac55521b-c307-4d59-b292-ed86f7f198bb',
              invocationId: secondInvocationId,
              callId: secondCallId,
              rawToolName: 'read_text_file'
            })
          ]
        }),
        uiStateJson: null
      }
    ],
    createdAt: 1,
    updatedAt: 10,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  })

  const timeline = (await loadConversation('conversation-mcp-trace-order'))?.messages[0].agentRun
    ?.timeline
  expect(timeline).toEqual([
    { id: 'message-before-first', type: 'message', content: '第一段。' },
    {
      id: `mcp-invocation-${firstInvocationId}`,
      type: 'mcp_tool_call',
      invocationId: firstInvocationId
    },
    { id: 'message-between-calls', type: 'message', content: '第二段。' },
    {
      id: `mcp-invocation-${secondInvocationId}`,
      type: 'mcp_tool_call',
      invocationId: secondInvocationId
    },
    { id: 'message-after-second', type: 'message', content: '第三段。' }
  ])
})

it('removes legacy generic MCP bodies using a valid approval identity even without an invocation', async () => {
  const callId = `tc1_${'b'.repeat(43)}`
  const canary = 'MCP_APPROVAL_ONLY_BODY_CANARY'
  storage.loadConversation.mockResolvedValueOnce({
    id: 'conversation-mcp-approval-only',
    projectId: null,
    modelId: 'model-1',
    title: 'MCP approval recovery',
    messages: [
      {
        id: 'assistant-mcp-approval-only',
        role: 'assistant',
        content: '',
        createdAt: 1,
        status: 'error',
        attachments: [],
        agentRunJson: JSON.stringify({
          runId: 'run-mcp-approval-only',
          status: 'failed',
          toolDefinitions: [],
          toolCalls: [
            {
              id: callId,
              tool: 'mcp__filesystem_test__move_file',
              args: { rawArguments: canary },
              approvalStatus: 'required'
            }
          ],
          toolResults: [
            {
              callId,
              tool: 'mcp__filesystem_test__move_file',
              ok: false,
              error: canary
            }
          ],
          approvals: [storedMcpApprovalAction(callId)],
          diffs: [],
          timeline: [{ id: `tool-call-${callId}`, type: 'tool_call', callId }]
        }),
        uiStateJson: null
      }
    ],
    createdAt: 1,
    updatedAt: 2,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  })

  const restoredRun = (await loadConversation('conversation-mcp-approval-only'))?.messages[0]
    .agentRun
  expect(restoredRun?.approvals).toEqual([])
  expect(restoredRun?.toolCalls).toEqual([])
  expect(restoredRun?.toolResults).toEqual([])
  expect(restoredRun?.timeline).toEqual([])
  expect(JSON.stringify(restoredRun)).not.toContain(canary)
})

it('deduplicates legacy invocation identities and preserves the authoritative terminal projection', async () => {
  const actionId = '94c2f39c-ddaa-49bb-a3ef-8756053d68c8'
  const invocationId = 'a8a6102c-8ad6-45d5-bb0d-3e4f0ad2a30f'
  const serverId = 'ce18d23c-e74f-4e89-8695-ce1e7c60ec92'
  const callId = `tc1_${'c'.repeat(43)}`
  storage.loadConversation.mockResolvedValueOnce({
    id: 'conversation-mcp-duplicate',
    projectId: null,
    modelId: 'model-1',
    title: 'MCP duplicate recovery',
    messages: [
      {
        id: 'assistant-mcp-duplicate',
        role: 'assistant',
        content: '',
        createdAt: 1,
        status: 'error',
        attachments: [],
        agentRunJson: JSON.stringify({
          runId: 'run-mcp-duplicate',
          status: 'failed',
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          diffs: [],
          timeline: [],
          mcpInvocations: [
            {
              actionId,
              invocationId,
              callId,
              serverId,
              serverDisplayName: 'Filesystem Test',
              rawToolName: 'move_file',
              modelToolName: 'mcp__filesystem_test__move_file',
              external: true,
              state: 'running',
              dispatchCertainty: 'possibly_dispatched',
              outputTruncated: false
            },
            {
              actionId,
              invocationId,
              callId,
              serverId,
              serverDisplayName: 'Filesystem Test',
              rawToolName: 'move_file',
              modelToolName: 'mcp__filesystem_test__move_file',
              external: true,
              state: 'completed',
              dispatchCertainty: 'response_received',
              outcome: 'succeeded',
              isError: false,
              durationMs: 25,
              outputTruncated: false
            }
          ]
        }),
        uiStateJson: null
      }
    ],
    createdAt: 1,
    updatedAt: 2,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  })

  const invocations = (await loadConversation('conversation-mcp-duplicate'))?.messages[0].agentRun
    ?.mcpInvocations
  expect(invocations).toHaveLength(1)
  expect(invocations?.[0]).toMatchObject({
    invocationId,
    state: 'completed',
    dispatchCertainty: 'response_received',
    outcome: 'succeeded'
  })
})
