import { expect, it, vi } from 'vitest'
import type { AgentCommandArtifactObservation, AgentProposedAction } from '@mycopilot/protocol'
import type { ChatMessage } from '../chatTypes'

const storage = vi.hoisted(() => ({
  loadConversation: vi.fn(),
  saveChatMessageState: vi.fn()
}))

vi.mock('../../../host/hostClient', () => ({
  hostClient: { storage }
}))

const { loadConversation, saveChatMessageState } = await import('../../storage/storageClient')
const { parsePersistedAgentRun } = await import('../../storage/persistedAgentRun')

const artifactCoverage = {
  rootsScanned: 1,
  directoryEntriesScanned: 1,
  officeFilesSeen: 1,
  filesHashed: 1,
  filesUnhashed: 0,
  bytesHashed: 1024,
  symlinksSkipped: 0,
  excludedDirectories: 0,
  durationMs: 1,
  timeBudgetExceeded: false,
  cancelled: false,
  truncated: false
}

const artifactObservation: AgentCommandArtifactObservation = {
  schemaVersion: 3,
  status: 'complete',
  partial: false,
  stopReasons: [],
  scanned: 1,
  returned: 1,
  omitted: 0,
  coverage: {
    workspaceIncluded: true,
    expectedOutputCount: 1,
    additionalRootCount: 0,
    before: { ...artifactCoverage, officeFilesSeen: 0, filesHashed: 0, bytesHashed: 0 },
    after: artifactCoverage
  },
  changes: [
    {
      kind: 'created',
      artifactKind: 'presentation',
      path: 'edited.pptx',
      scope: 'workspace',
      after: { sizeBytes: 1024, validation: { status: 'valid' } }
    }
  ],
  changesTruncated: false,
  changesOmitted: 0,
  expectedOutputs: [
    {
      requestedPath: 'edited.pptx',
      outcome: 'created',
      path: 'edited.pptx',
      scope: 'workspace',
      artifactKind: 'presentation',
      metadata: { sizeBytes: 1024, validation: { status: 'valid' } }
    }
  ],
  warnings: []
}

function storedMcpApprovalAction(
  callId: string
): Extract<AgentProposedAction, { type: 'mcp_tool_call' }> {
  const serverId = 'ce18d23c-e74f-4e89-8695-ce1e7c60ec92'
  return {
    type: 'mcp_tool_call' as const,
    approval: {
      identity: {
        actionId: '94c2f39c-ddaa-49bb-a3ef-8756053d68c8',
        invocationId: 'a8a6102c-8ad6-45d5-bb0d-3e4f0ad2a30f',
        runId: 'run-mcp-approval-only',
        callId,
        provenance: {
          serverId,
          scope: { type: 'user' as const },
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
        approvalStatus: 'required',
        reason: null
      },
      summary: {
        serverId,
        serverDisplayName: 'Filesystem Test',
        scope: { type: 'user' },
        rawToolName: 'move_file',
        modelToolName: 'mcp__filesystem_test__move_file',
        displayReason: null,
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
      startedAt: 1,
      completedAt: 2,
      toolDefinitions: [],
      toolCalls: [
        {
          id: 'command-call',
          tool: 'run_command',
          args: { command: 'printf done' },
          approvalStatus: 'not_required',
          reason: null
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
      llmRetry: {
        category: 'rate_limited',
        providerCode: 'rate_limit_exceeded',
        delayMs: 5_000,
        retryAt: 10_000,
        attempt: 2,
        maxAttempts: 3
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
  expect(storedRun.llmRetry).toBeUndefined()
  expect(storedRun.toolResults).toEqual(message.agentRun?.toolResults)
})

it('preserves a multiline command exactly through persistence and hydration', async () => {
  const command = "python3 <<'PY'\nif True:\n    print('Aspen PDF')\nPY\n"
  const message: ChatMessage = {
    id: 'assistant-multiline-command',
    role: 'assistant',
    content: 'Command complete.',
    createdAt: 1,
    status: 'sent',
    agentRun: {
      runId: 'run-multiline-command',
      status: 'completed',
      startedAt: 1,
      completedAt: 2,
      toolDefinitions: [],
      toolCalls: [
        {
          id: 'multiline-command-call',
          tool: 'run_command',
          args: { command, reason: '读取 PDF' },
          approvalStatus: 'approved',
          reason: null
        }
      ],
      toolResults: [],
      approvals: [],
      diffs: [],
      timeline: [
        {
          id: 'tool-call-multiline-command-call',
          type: 'tool_call',
          callId: 'multiline-command-call'
        }
      ]
    }
  }

  storage.saveChatMessageState.mockResolvedValueOnce(undefined)
  await saveChatMessageState('conversation-multiline-command', message)
  const storedMessage = storage.saveChatMessageState.mock.calls.at(-1)?.[0]?.message
  const persisted = JSON.parse(storedMessage.agentRunJson)
  expect(persisted.toolCalls[0].args.command).toBe(command)

  storage.loadConversation.mockResolvedValueOnce({
    id: 'conversation-multiline-command',
    projectId: null,
    modelId: 'model-1',
    title: 'Multiline command persistence',
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
    updatedAt: 2,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  })

  const restored = await loadConversation('conversation-multiline-command')
  expect(restored?.messages[0].agentRun?.toolCalls[0]?.args).toEqual({
    command,
    reason: '读取 PDF'
  })
})

it('rejects a nonterminal command session in durable chat state', async () => {
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
              approvalStatus: 'approved',
              reason: null
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

  await expect(loadConversation('conversation-command-reload')).rejects.toThrow(
    'Stored Agent run is malformed'
  )
})

it('persists and reloads only immutable command terminal metadata', async () => {
  const publishedHash = 'a'.repeat(64)
  const publishedOutput = {
    name: 'pages/page-1.png',
    kind: 'image' as const,
    readPath: `image-artifact://sha256/${publishedHash}`,
    mimeType: 'image/png',
    sizeBytes: 2048,
    sha256: publishedHash,
    width: 1200,
    height: 1600
  }
  const message: ChatMessage = {
    id: 'assistant-command-terminal',
    role: 'assistant',
    content: 'The application exited successfully.',
    createdAt: 1,
    status: 'sent',
    agentRun: {
      runId: 'run-command-terminal',
      status: 'completed',
      startedAt: 1,
      completedAt: 20,
      toolDefinitions: [],
      toolCalls: [
        {
          id: 'command-call',
          tool: 'run_command',
          args: { command: 'long-running' },
          approvalStatus: 'approved',
          reason: null
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
          outputTruncated: false,
          artifactObservation,
          outputs: [publishedOutput]
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
      outputTruncated: false,
      artifactObservation,
      outputs: [publishedOutput]
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

it('rejects malformed artifact observations in durable command Session state', () => {
  const base = currentStoredRun({
    runId: 'run-command-observation',
    toolCalls: [
      {
        id: 'command-call',
        tool: 'run_command',
        args: { command: 'node editor.mjs' },
        approvalStatus: 'approved',
        reason: null
      }
    ],
    timeline: [{ id: 'tool-call-command-call', type: 'tool_call', callId: 'command-call' }],
    commandSessions: {
      'command-call': {
        callId: 'command-call',
        status: 'exited',
        endedAt: 12,
        exitCode: 0,
        latestSequence: 1,
        outputTruncated: false,
        artifactObservation: { ...artifactObservation, schemaVersion: 999 }
      }
    }
  })

  expect(parsePersistedAgentRun(base)).toBeUndefined()
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
      startedAt: 1,
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

const actionId = '94c2f39c-ddaa-49bb-a3ef-8756053d68c8'
const invocationId = 'a8a6102c-8ad6-45d5-bb0d-3e4f0ad2a30f'
const serverId = 'ce18d23c-e74f-4e89-8695-ce1e7c60ec92'
const callId = `tc1_${'a'.repeat(43)}`

function currentStoredRun(overrides: Record<string, unknown> = {}) {
  return {
    runId: 'run-current',
    status: 'completed',
    startedAt: 1,
    completedAt: 10,
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    webSearchActivities: [],
    readActivities: [],
    approvals: [],
    diffs: [],
    fileDrafts: [],
    mcpInvocations: [],
    messageStreamCheckpoints: {},
    timeline: [],
    ...overrides
  }
}

it('persists and restores a safe model request interruption without provider diagnostics', async () => {
  const message: ChatMessage = {
    id: 'assistant-safe-interruption',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'sent',
    agentRun: {
      runId: 'run-safe-interruption',
      status: 'failed',
      startedAt: 1,
      completedAt: 2,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      diffs: [],
      timeline: [],
      interruption: { reason: 'service_connection_failed' }
    }
  }

  storage.saveChatMessageState.mockResolvedValueOnce(undefined)
  await saveChatMessageState('conversation-safe-interruption', message)

  const storedMessage = storage.saveChatMessageState.mock.calls.at(-1)?.[0]?.message
  const persisted = JSON.parse(storedMessage.agentRunJson)
  expect(persisted.interruption).toEqual({ reason: 'service_connection_failed' })
  expect(JSON.stringify(persisted)).not.toContain('provider')

  storage.loadConversation.mockResolvedValueOnce(
    storedConversation(persisted, 'conversation-safe-interruption')
  )
  const restored = await loadConversation('conversation-safe-interruption')
  expect(restored?.messages[0].agentRun?.interruption).toEqual({
    reason: 'service_connection_failed'
  })
})

it('rejects unknown persisted model request interruption reasons', () => {
  expect(
    parsePersistedAgentRun(
      currentStoredRun({ interruption: { reason: 'raw_provider_internal_failure' } })
    )
  ).toBeUndefined()
})

function storedConversation(agentRun: Record<string, unknown>, id = 'conversation-current') {
  return {
    id,
    projectId: null,
    modelId: 'model-1',
    title: 'Current persisted run',
    messages: [
      {
        id: 'assistant-current',
        role: 'assistant',
        content: 'partial',
        createdAt: 1,
        status: 'error',
        attachments: [],
        agentRunJson: JSON.stringify(agentRun),
        uiStateJson: null
      }
    ],
    createdAt: 1,
    updatedAt: 10,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  }
}

function runningMcpInvocation() {
  return {
    actionId,
    invocationId,
    callId,
    serverId,
    serverDisplayName: 'Filesystem Test',
    scope: { type: 'user' as const },
    rawToolName: 'move_file',
    modelToolName: 'mcp__filesystem_test__move_file',
    displayReason: 'Move the approved fixture file',
    external: true as const,
    state: 'running',
    dispatchCertainty: 'possibly_dispatched',
    outputTruncated: false
  }
}

function currentOfficeApproval() {
  return {
    type: 'office_operation',
    officeOperation: {
      schemaVersion: 6,
      id: 'office-validate',
      semanticArgs: { operation: 'validate', filePath: 'report.docx' },
      prepared: {
        schemaVersion: 6,
        providerId: 'officecli',
        engineRevision: 'sha256:engine',
        workspaceRevision: null,
        access: 'readOnly',
        request: {
          documentKind: 'document',
          operation: 'validate',
          documentPath: 'report.docx',
          outputPath: null,
          destinationPath: null,
          inputs: [],
          timeoutMs: null,
          parameters: { type: 'validate' }
        },
        argv: ['validate', 'report.docx'],
        resolvedRenderPlan: null,
        paths: [],
        inputBindings: []
      },
      approvalStatus: 'required',
      reason: 'Validate the document'
    }
  }
}

function currentPresentationRenderApproval() {
  const current = currentOfficeApproval()
  return {
    ...current,
    officeOperation: {
      ...current.officeOperation,
      id: 'office-render',
      semanticArgs: {
        operation: 'render',
        filePath: 'deck.pptx',
        outputPath: 'preview.png'
      },
      prepared: {
        ...current.officeOperation.prepared,
        access: 'fileWrite',
        request: {
          documentKind: 'presentation',
          operation: 'view',
          documentPath: 'deck.pptx',
          outputPath: 'preview.png',
          destinationPath: null,
          inputs: [],
          timeoutMs: null,
          parameters: {
            type: 'view',
            mode: 'screenshot',
            pages: [{ start: 1, end: 3 }],
            grid: { mode: 'auto' }
          }
        },
        argv: [
          'view',
          'deck.pptx',
          'screenshot',
          '--page',
          '1-3',
          '--grid',
          '2',
          '--screenshot-width',
          '1600',
          '--screenshot-height',
          '922',
          '--render',
          'html',
          '--json',
          '-o',
          'preview.png'
        ],
        resolvedRenderPlan: {
          requestedPages: [1, 2, 3],
          slideWidthEmu: 12_192_000,
          slideHeightEmu: 6_858_000,
          viewport: { width: 1600, height: 922 },
          grid: { mode: 'columns', columns: 2 }
        }
      },
      reason: 'Render the presentation'
    }
  }
}

function currentSkillInstallationApproval() {
  return {
    type: 'skill_installation',
    installation: {
      schemaVersion: 1,
      id: 'skill-install-action-1',
      installRef: `skill_install_${'a'.repeat(32)}`,
      preview: {
        name: 'example-skill',
        description: 'Example Skill',
        sourceSummary: { kind: 'githubRepository' },
        resolvedRevision: 'b'.repeat(40),
        fileCount: 1,
        totalBytes: 100,
        resourceSummary: { total: 0, references: 0, assets: 0, scripts: 0, bytes: 0 },
        containsScripts: false,
        warnings: [],
        compatibility: 'compatible',
        operation: 'install',
        impact: 'addManagedSkill'
      },
      approvalStatus: 'required',
      expiresAt: 1_753_844_100_000
    }
  }
}

it('accepts only the current command approval projection and rejects runtime authority fields', () => {
  const command = {
    type: 'command',
    command: {
      id: 'command-current',
      command: 'cargo test',
      cwd: null,
      timeoutMs: 30_000,
      approvalStatus: 'required',
      riskLevel: 'read_only',
      reason: 'Run tests',
      observe: null
    }
  }
  expect(parsePersistedAgentRun(currentStoredRun({ approvals: [command] }))).toBeDefined()
  expect(
    parsePersistedAgentRun(
      currentStoredRun({
        approvals: [{ ...command, command: { ...command.command, runtime: { kind: 'python' } } }]
      })
    )
  ).toBeUndefined()
})

it('rejects removed Office arguments and precondition fields at their nested boundaries', () => {
  const current = currentOfficeApproval()
  expect(parsePersistedAgentRun(currentStoredRun({ approvals: [current] }))).toBeDefined()
  expect(
    parsePersistedAgentRun(
      currentStoredRun({
        approvals: [
          {
            ...current,
            officeOperation: { ...current.officeOperation, arguments: { operation: 'validate' } }
          }
        ]
      })
    )
  ).toBeUndefined()
  expect(
    parsePersistedAgentRun(
      currentStoredRun({
        approvals: [
          {
            ...current,
            officeOperation: {
              ...current.officeOperation,
              prepared: { ...current.officeOperation.prepared, precondition: { revision: 'old' } }
            }
          }
        ]
      })
    )
  ).toBeUndefined()
})

it('strictly validates frozen presentation render-plan bounds and schema identity', () => {
  const current = currentPresentationRenderApproval()
  expect(parsePersistedAgentRun(currentStoredRun({ approvals: [current] }))).toBeDefined()

  for (const resolvedRenderPlan of [
    null,
    { ...current.officeOperation.prepared.resolvedRenderPlan, requestedPages: [1, 1] },
    { ...current.officeOperation.prepared.resolvedRenderPlan, requestedPages: [10_001] },
    {
      ...current.officeOperation.prepared.resolvedRenderPlan,
      requestedPages: Array.from({ length: 129 }, (_, index) => index + 1)
    },
    { ...current.officeOperation.prepared.resolvedRenderPlan, slideWidthEmu: 0x1_0000_0000 },
    {
      ...current.officeOperation.prepared.resolvedRenderPlan,
      viewport: { width: 16_385, height: 922 }
    },
    { ...current.officeOperation.prepared.resolvedRenderPlan, grid: { mode: 'auto' } },
    {
      ...current.officeOperation.prepared.resolvedRenderPlan,
      grid: { mode: 'columns', columns: 33 }
    }
  ]) {
    expect(
      parsePersistedAgentRun(
        currentStoredRun({
          approvals: [
            {
              ...current,
              officeOperation: {
                ...current.officeOperation,
                prepared: { ...current.officeOperation.prepared, resolvedRenderPlan }
              }
            }
          ]
        })
      )
    ).toBeUndefined()
  }

  expect(
    parsePersistedAgentRun(
      currentStoredRun({
        approvals: [
          {
            ...current,
            officeOperation: { ...current.officeOperation, schemaVersion: 5 }
          }
        ]
      })
    )
  ).toBeUndefined()

  expect(
    parsePersistedAgentRun(
      currentStoredRun({
        approvals: [
          {
            ...current,
            officeOperation: {
              ...current.officeOperation,
              prepared: { ...current.officeOperation.prepared, schemaVersion: 5 }
            }
          }
        ]
      })
    )
  ).toBeUndefined()
})

it('preserves Host-verified Office render layout geometry in the durable run projection', () => {
  const officeCallId = 'office-render-result'
  const layoutCoverage = {
    requestedPages: [1, 2, 3],
    evidence: 'trustedRendererGeometry',
    grid: {
      columns: 2,
      rows: 2,
      viewportWidth: 1600,
      viewportHeight: 922,
      contentWidth: 1600,
      contentHeight: 922
    }
  }
  const parsed = parsePersistedAgentRun(
    currentStoredRun({
      toolCalls: [
        {
          id: officeCallId,
          tool: 'office_presentation',
          args: { operation: 'render', filePath: 'deck.pptx' },
          approvalStatus: 'not_required',
          reason: null
        }
      ],
      toolResults: [
        {
          callId: officeCallId,
          tool: 'office_presentation',
          ok: true,
          result: {
            outputs: [
              {
                role: 'render',
                kind: 'image',
                readPath: 'preview.png',
                layoutCoverage
              }
            ]
          }
        }
      ]
    })
  )

  expect(parsed?.toolResults[0]?.result).toMatchObject({
    outputs: [{ readPath: 'preview.png', layoutCoverage }]
  })
})

it('strictly validates the current Skill installation action instead of accepting any object', () => {
  const current = currentSkillInstallationApproval()
  expect(parsePersistedAgentRun(currentStoredRun({ approvals: [current] }))).toBeDefined()
  const missingInstallRef = { ...current.installation }
  Reflect.deleteProperty(missingInstallRef, 'installRef')
  expect(
    parsePersistedAgentRun(
      currentStoredRun({
        approvals: [{ ...current, installation: missingInstallRef }]
      })
    )
  ).toBeUndefined()
  expect(
    parsePersistedAgentRun(
      currentStoredRun({
        approvals: [
          {
            ...current,
            installation: {
              ...current.installation,
              preview: { ...current.installation.preview, packagePath: '/old/path' }
            }
          }
        ]
      })
    )
  ).toBeUndefined()
})

it('settles a valid nonterminal MCP child when its current parent run is terminal', async () => {
  storage.loadConversation.mockResolvedValueOnce(
    storedConversation(
      currentStoredRun({
        status: 'failed',
        mcpInvocations: [runningMcpInvocation()],
        timeline: [
          {
            id: `mcp-invocation-${invocationId}`,
            type: 'mcp_tool_call',
            invocationId
          }
        ]
      })
    )
  )

  const restored = await loadConversation('conversation-current')
  expect(restored?.messages[0].agentRun?.mcpInvocations?.[0]).toMatchObject({
    state: 'outcome_unknown',
    outcome: 'outcome_unknown',
    dispatchCertainty: 'possibly_dispatched',
    errorCode: 'mcp.tool_outcome_unknown',
    displayReason: 'Move the approved fixture file'
  })
})

it('persists the current safe MCP projection and excludes its approval payload', async () => {
  const message: ChatMessage = {
    id: 'assistant-mcp-current',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun: {
      runId: 'run-mcp-current',
      status: 'waiting_for_approval',
      startedAt: 1,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [storedMcpApprovalAction(callId)],
      diffs: [],
      mcpInvocations: [
        {
          ...runningMcpInvocation(),
          state: 'pending_approval',
          dispatchCertainty: 'definitely_not_dispatched'
        }
      ],
      timeline: [
        {
          id: `mcp-invocation-${invocationId}`,
          type: 'mcp_tool_call',
          invocationId
        }
      ]
    }
  }
  storage.saveChatMessageState.mockResolvedValueOnce(undefined)
  await saveChatMessageState('conversation-mcp-current', message)

  const storedMessage = storage.saveChatMessageState.mock.calls.at(-1)?.[0]?.message
  const persisted = JSON.parse(storedMessage.agentRunJson)
  expect(persisted.approvals).toEqual([])
  expect(persisted.mcpInvocations).toEqual(message.agentRun?.mcpInvocations)
  expect(JSON.stringify(persisted)).not.toContain('payloadPersistence')

  storage.loadConversation.mockResolvedValueOnce(
    storedConversation(persisted, 'conversation-mcp-current')
  )
  const restored = await loadConversation('conversation-mcp-current')
  expect(restored?.messages[0].agentRun?.mcpInvocations).toEqual(message.agentRun?.mcpInvocations)
})

it('rejects a nonempty persisted run with missing current fields', async () => {
  storage.loadConversation.mockResolvedValueOnce(
    storedConversation({ runId: 'run-incomplete', status: 'failed' })
  )
  await expect(loadConversation('conversation-current')).rejects.toThrow(
    'Stored Agent run is malformed'
  )
})

it('accepts the current pre-start projection before the Host assigns a run id', async () => {
  const preStart = {
    ...currentStoredRun({ runId: null, status: 'starting' }),
    completedAt: undefined
  }
  storage.loadConversation.mockResolvedValueOnce(storedConversation(preStart))

  const restored = await loadConversation('conversation-current')
  expect(restored?.messages[0].agentRun?.runId).toBeNull()
  expect(restored?.messages[0].agentRun?.status).toBe('starting')
})

it('rejects malformed persisted web search sources instead of accepting an old partial shape', async () => {
  storage.loadConversation.mockResolvedValueOnce(
    storedConversation(
      currentStoredRun({
        webSearchActivities: [
          {
            callId: 'search-call',
            query: 'current query',
            provider: 'current-provider',
            status: 'completed',
            sources: [
              {
                id: 'source-1',
                title: 'Missing current source fields'
              }
            ],
            updatedAt: 10
          }
        ]
      })
    )
  )
  await expect(loadConversation('conversation-current')).rejects.toThrow(
    'Stored Agent run is malformed'
  )
})

it('rejects extra MCP persisted fields instead of stripping them', async () => {
  storage.loadConversation.mockResolvedValueOnce(
    storedConversation(
      currentStoredRun({
        mcpInvocations: [{ ...runningMcpInvocation(), rawArguments: 'must-not-survive' }],
        timeline: [
          {
            id: `mcp-invocation-${invocationId}`,
            type: 'mcp_tool_call',
            invocationId
          }
        ]
      })
    )
  )
  await expect(loadConversation('conversation-current')).rejects.toThrow(
    'Stored Agent run is malformed'
  )
})

it('rejects duplicate MCP identities instead of choosing one projection', async () => {
  storage.loadConversation.mockResolvedValueOnce(
    storedConversation(
      currentStoredRun({
        mcpInvocations: [runningMcpInvocation(), runningMcpInvocation()],
        timeline: [
          {
            id: `mcp-invocation-${invocationId}`,
            type: 'mcp_tool_call',
            invocationId
          }
        ]
      })
    )
  )
  await expect(loadConversation('conversation-current')).rejects.toThrow(
    'Stored Agent run is malformed'
  )
})

it('rejects generic and MCP cross-projections instead of rewriting the Timeline', async () => {
  storage.loadConversation.mockResolvedValueOnce(
    storedConversation(
      currentStoredRun({
        toolCalls: [
          {
            id: callId,
            tool: 'mcp__filesystem_test__move_file',
            args: {},
            approvalStatus: 'approved',
            reason: null
          }
        ],
        mcpInvocations: [runningMcpInvocation()],
        timeline: [
          { id: `tool-call-${callId}`, type: 'tool_call', callId },
          {
            id: `mcp-invocation-${invocationId}`,
            type: 'mcp_tool_call',
            invocationId
          }
        ]
      })
    )
  )
  await expect(loadConversation('conversation-current')).rejects.toThrow(
    'Stored Agent run is malformed'
  )
})
