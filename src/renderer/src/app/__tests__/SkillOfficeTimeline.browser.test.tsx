// Renderer browser regressions for persisted Skill/Office timeline activity and Office files.

import type {
  AgentCommandArtifactObservation,
  AgentToolCall,
  AgentToolResult
} from '@mycopilot/protocol'
import type { ChatMessage } from '../../features/chat/chatTypes'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ChatMessageItem } from '../../features/chat/components/ChatMessageItem'

const { readArtifact, revealStoredProjectFile } = vi.hoisted(() => ({
  readArtifact: vi.fn(),
  revealStoredProjectFile: vi.fn().mockResolvedValue(undefined)
}))
const translations: Record<string, string> = {
  'agent.processed': '已处理 {duration}',
  'agent.skill.loaded': '已加载工具',
  'agent.skill.loadedItem': '读取 {skill} 技能',
  'agent.skill.reference': '资料',
  'agent.skill.template': '模板',
  'agent.skill.capability': '能力',
  'agent.skill.actionRead': '读取',
  'agent.skill.actionUse': '使用',
  'agent.skill.resource.completed': '已{action}「{skill}」技能{subject}',
  'agent.office.kind.spreadsheet': '电子表格',
  'agent.office.kind.document': '文档',
  'agent.office.kind.presentation': '演示文稿',
  'agent.office.failed': '{kind}处理失败',
  'agent.office.cancelled': '已取消{kind}处理',
  'agent.office.rejected': '已拒绝{kind}处理',
  'agent.office.conflict': '{kind}存在冲突',
  'agent.office.edited': '已编辑{kind}',
  'agent.office.created': '已创建{kind}',
  'agent.office.viewed': '已查看{kind}信息',
  'agent.office.groupOperations': '{label} · {count} 项操作',
  'agent.office.groupChecks': '{label} · {count} 项检查',
  'agent.office.groupOperationSummary': '{kind}操作',
  'agent.office.groupCheckSummary': '{kind}检查',
  'agent.office.groupStatus.waiting': '等待审批 {count} 项',
  'agent.office.groupStatus.running': '进行中 {count} 项',
  'agent.office.groupStatus.completed': '成功 {count} 项',
  'agent.office.groupStatus.failed': '失败 {count} 项',
  'agent.office.groupStatus.conflict': '冲突 {count} 项',
  'agent.office.groupStatus.rejected': '拒绝 {count} 项',
  'agent.office.groupStatus.cancelled': '取消 {count} 项',
  'agent.office.itemStatus.waiting': '等待审批',
  'agent.office.itemStatus.running': '进行中',
  'agent.office.itemStatus.completed': '成功',
  'agent.office.itemStatus.failed': '失败',
  'agent.office.itemStatus.conflict': '冲突',
  'agent.office.itemStatus.rejected': '已拒绝',
  'agent.office.itemStatus.cancelled': '已取消',
  'agent.office.files': 'Office 文件',
  'agent.office.reveal': '在文件夹中打开',
  'agent.office.revealUnavailable': '无法定位',
  'imagePreview.download': '下载',
  'skills.bundled.spreadsheets.name': '电子表格',
  'skills.bundled.spreadsheets.description': '创建、编辑、计算、渲染和验证 Excel 工作簿。'
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => translations[key] ?? key
  })
}))

vi.mock('../../features/storage/storageClient', () => ({
  loadAttachmentImage: vi.fn(),
  loadImageFile: vi.fn(),
  revealStoredProjectFile
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: { imageGeneration: { readArtifact } }
}))

function assistantMessage(overrides: Partial<ChatMessage['agentRun']> = {}): ChatMessage {
  return {
    id: 'assistant-message',
    role: 'assistant',
    content: '完成。',
    createdAt: 1,
    status: 'sent',
    uiState: { timelineCollapsed: false },
    agentRun: {
      runId: 'run-1',
      status: 'completed',
      startedAt: 1,
      completedAt: 2,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline: [],
      ...overrides
    }
  }
}

function presentationArtifactObservation(path: string): AgentCommandArtifactObservation {
  const snapshotCoverage = {
    rootsScanned: 1,
    directoryEntriesScanned: 1,
    officeFilesSeen: 1,
    filesHashed: 1,
    filesUnhashed: 0,
    bytesHashed: 4096,
    symlinksSkipped: 0,
    excludedDirectories: 0,
    durationMs: 1,
    timeBudgetExceeded: false,
    cancelled: false,
    truncated: false
  }
  const metadata = {
    sizeBytes: 4096,
    sha256: 'd'.repeat(64),
    validation: { status: 'valid' as const }
  }
  return {
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
      before: snapshotCoverage,
      after: snapshotCoverage
    },
    changes: [
      {
        kind: 'created',
        artifactKind: 'presentation',
        path,
        scope: 'workspace',
        after: metadata
      }
    ],
    changesTruncated: false,
    changesOmitted: 0,
    expectedOutputs: [
      {
        requestedPath: path,
        outcome: 'created',
        path,
        scope: 'workspace',
        artifactKind: 'presentation',
        metadata
      }
    ],
    warnings: []
  }
}

function imageGenerationCall(id: string, reason: string): AgentToolCall {
  return {
    id,
    tool: 'image_generation',
    approvalStatus: 'not_required',
    reason: null,
    args: {
      request: { operation: 'generate', hasInputImage: false },
      reason
    }
  }
}

function imageGenerationResult(id: string, hash: string): AgentToolResult {
  return {
    callId: id,
    tool: 'image_generation',
    ok: true,
    result: {
      schemaVersion: 1,
      status: 'succeeded',
      operation: 'generate',
      artifact: {
        artifactId: `sha256:${hash}`,
        uri: `image-artifact://sha256/${hash}`,
        kind: 'image',
        format: 'jpeg',
        mimeType: 'image/jpeg',
        width: 1200,
        height: 800,
        sizeBytes: 4096,
        sha256: hash
      },
      audit: {
        executionId: `agent-v1:${'a'.repeat(64)}`,
        requestFingerprint: `sha256:${'b'.repeat(64)}`,
        providerProfileId: 'default',
        adapterId: 'smartmlSeedream',
        profileRevision: 1,
        modelId: 'image-model',
        createdAt: 1,
        completedAt: 2,
        durationMs: 1
      }
    }
  }
}

describe('terminal assistant answer timeline', () => {
  const modes = ['interactive', 'observer'] as const
  const streamId = 'run-1-final-stream'
  const finalContent = '三个子智能体目前全部等待审批。你那边审批一下，我继续等。'
  const partialContent = '三个子智能体目前全部等待审批。你'

  function terminalMessage(overrides: Partial<ChatMessage['agentRun']> = {}): ChatMessage {
    const message = assistantMessage({
      toolCalls: [
        {
          id: 'terminal-check',
          tool: 'run_command',
          approvalStatus: 'not_required',
          reason: null,
          args: { command: 'true', reason: '检查结束。' }
        }
      ],
      toolResults: [
        {
          callId: 'terminal-check',
          tool: 'run_command',
          ok: true,
          result: { exitCode: 0, stdout: '', stderr: '' }
        }
      ],
      ...overrides
    })
    message.content = finalContent
    return message
  }

  function visibleNarration(container: HTMLElement) {
    return Array.from(container.querySelectorAll('.chat-agent-text'), (element) =>
      element.textContent?.trim()
    )
  }

  for (const mode of modes) {
    it(`does not turn prior narration into an empty completed answer in ${mode}`, async () => {
      const narration = '过程说明保留在时间线中。'
      const message = terminalMessage({
        timeline: [
          { id: 'trace-message-0', type: 'message', content: narration, traceSequence: 0 },
          {
            id: 'tool-call-terminal-check',
            type: 'tool_call',
            callId: 'terminal-check',
            traceSequence: 1
          }
        ]
      })
      message.content = ''
      const screen = await render(
        <ChatMessageItem message={message} mode={mode} showTokenUsageDetails={false} />
      )
      expect(visibleNarration(screen.container)).toEqual([narration])
      await screen.rerender(
        <ChatMessageItem
          message={message}
          mode={mode}
          showTokenUsageDetails={false}
          timelineCollapsedOverride
        />
      )
      expect(visibleNarration(screen.container)).toEqual([])
    })

    it.each([true, false])(
      `shows the complete answer once in ${mode} with a stale final stream (checkpoint=%s)`,
      async (hasCheckpoint) => {
        const narration = '超时了，先查一下当前状态。'
        const message = terminalMessage({
          messageStreamCheckpoints: hasCheckpoint
            ? { [streamId]: { previousContent: narration } }
            : {},
          timeline: [
            { id: 'trace-message-1', type: 'message', content: narration, traceSequence: 1 },
            {
              id: 'tool-call-terminal-check',
              type: 'tool_call',
              callId: 'terminal-check',
              traceSequence: 2
            },
            {
              id: `message-stream-${streamId}`,
              type: 'message',
              content: partialContent,
              streamId
            }
          ]
        })
        const screen = await render(
          <ChatMessageItem message={message} mode={mode} showTokenUsageDetails={false} />
        )

        expect(visibleNarration(screen.container)).toEqual([narration, finalContent])

        await screen.rerender(
          <ChatMessageItem
            message={message}
            mode={mode}
            showTokenUsageDetails={false}
            timelineCollapsedOverride
          />
        )
        expect(visibleNarration(screen.container)).toEqual([finalContent])

        await screen.rerender(
          <ChatMessageItem message={message} mode={mode} showTokenUsageDetails={false} />
        )
        expect(visibleNarration(screen.container)).toEqual([narration, finalContent])
      }
    )

    it.each(['equal', 'prefix'] as const)(
      `retains committed narration in ${mode} even when its text is a final-answer %s`,
      async (relationship) => {
        const narration = relationship === 'equal' ? finalContent : partialContent
        const message = terminalMessage({
          messageStreamCheckpoints: {},
          timeline: [
            {
              id: 'message-stream-committed-narration',
              type: 'message',
              content: narration,
              streamId: 'committed-narration',
              traceSequence: 1
            },
            {
              id: 'tool-call-terminal-check',
              type: 'tool_call',
              callId: 'terminal-check',
              traceSequence: 2
            },
            {
              id: `message-stream-${streamId}`,
              type: 'message',
              content: partialContent,
              streamId
            }
          ]
        })
        const screen = await render(
          <ChatMessageItem message={message} mode={mode} showTokenUsageDetails={false} />
        )

        expect(visibleNarration(screen.container)).toEqual([narration, finalContent])

        await screen.rerender(
          <ChatMessageItem
            message={message}
            mode={mode}
            showTokenUsageDetails={false}
            timelineCollapsedOverride
          />
        )
        expect(visibleNarration(screen.container)).toEqual([finalContent])
      }
    )

    it.each(['cancelled', 'failed'] as const)(
      `retains interrupted partial text in the expanded ${mode} timeline (status=%s)`,
      async (status) => {
        const message = terminalMessage({
          status,
          messageStreamCheckpoints: { [streamId]: { previousContent: '' } },
          timeline: [
            {
              id: 'tool-call-terminal-check',
              type: 'tool_call',
              callId: 'terminal-check',
              traceSequence: 1
            },
            {
              id: `message-stream-${streamId}`,
              type: 'message',
              content: partialContent,
              streamId
            }
          ]
        })
        message.content = status === 'cancelled' ? '' : '请求失败。'
        const finalNarration = status === 'cancelled' ? [] : [message.content]
        const screen = await render(
          <ChatMessageItem message={message} mode={mode} showTokenUsageDetails={false} />
        )

        expect(visibleNarration(screen.container)).toEqual([partialContent, ...finalNarration])

        await screen.rerender(
          <ChatMessageItem
            message={message}
            mode={mode}
            showTokenUsageDetails={false}
            timelineCollapsedOverride
          />
        )
        expect(visibleNarration(screen.container)).toEqual(finalNarration)
      }
    )
  }
})

describe('Skill and Office chat timeline', () => {
  it('does not expose stale narration as a final answer when a cancelled timeline is collapsed', async () => {
    const message = assistantMessage({
      status: 'cancelled',
      toolCalls: [
        {
          id: 'cancelled-command',
          tool: 'run_command',
          approvalStatus: 'not_required',
          reason: '执行停止前的检查。',
          args: { command: 'sleep 30' }
        }
      ],
      timeline: [
        {
          id: 'cancelled-narration',
          type: 'message',
          content: '这只是停止前的过程说明，不是最终回复。'
        },
        { id: 'cancelled-tool', type: 'tool_call', callId: 'cancelled-command' }
      ]
    })
    // Simulate an older persisted record produced before cancellation cleared the live accumulator.
    message.content = '这只是停止前的过程说明，不是最终回复。'
    message.uiState = { timelineCollapsed: true }

    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    expect(screen.container.textContent).not.toContain('这只是停止前的过程说明，不是最终回复。')
    expect(screen.container.querySelector('.chat-agent-text')).toBeNull()
  })

  it('renders the durable final answer after a Trace-rebuilt expanded timeline', async () => {
    const message = assistantMessage({
      toolCalls: [
        {
          id: 'command-1',
          tool: 'run_command',
          approvalStatus: 'not_required',
          reason: null,
          args: { command: 'true', reason: '完成最后一次验证。' }
        }
      ],
      toolResults: [
        {
          callId: 'command-1',
          tool: 'run_command',
          ok: true,
          result: { exitCode: 0, stdout: '', stderr: '' }
        }
      ],
      timeline: [
        { id: 'trace-message-1', type: 'message', content: '这是过程旁白。' },
        { id: 'tool-call-command-1', type: 'tool_call', callId: 'command-1' }
      ]
    })
    message.content = '这是后端持久化的最终总结。'

    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    expect(screen.container.textContent).toContain('这是过程旁白。')
    expect(screen.container.textContent?.match(/这是后端持久化的最终总结。/g)).toHaveLength(1)
  })

  it('keeps MCP calls between their narration anchors from running through completion', async () => {
    const firstInvocationId = '22222222-2222-4222-8222-222222222222'
    const secondInvocationId = '44444444-4444-4444-8444-444444444444'
    const terminalMessage = assistantMessage({
      status: 'failed',
      mcpInvocations: [
        {
          actionId: '11111111-1111-4111-8111-111111111111',
          invocationId: firstInvocationId,
          callId: `tc1_${'a'.repeat(43)}`,
          serverId: '33333333-3333-4333-8333-333333333333',
          serverDisplayName: 'Filesystem Test',
          scope: { type: 'user' },
          rawToolName: 'move_file',
          modelToolName: 'mcp__filesystem_test__move_file',
          external: true,
          state: 'outcome_unknown',
          dispatchCertainty: 'possibly_dispatched',
          outcome: 'outcome_unknown',
          errorCode: 'mcp.tool_outcome_unknown',
          outputTruncated: false
        },
        {
          actionId: '55555555-5555-4555-8555-555555555555',
          invocationId: secondInvocationId,
          callId: `tc1_${'b'.repeat(43)}`,
          serverId: '33333333-3333-4333-8333-333333333333',
          serverDisplayName: 'Filesystem Test',
          scope: { type: 'user' },
          rawToolName: 'read_text_file',
          modelToolName: 'mcp__filesystem_test__read_text_file',
          external: true,
          state: 'completed',
          dispatchCertainty: 'response_received',
          outcome: 'succeeded',
          isError: false,
          durationMs: 8,
          outputTruncated: false
        }
      ],
      timeline: [
        { id: 'message-stream-1', type: 'message', content: '第一段。' },
        {
          id: `mcp-invocation-${firstInvocationId}`,
          type: 'mcp_tool_call',
          invocationId: firstInvocationId
        },
        { id: 'message-stream-2', type: 'message', content: '第二段。' },
        {
          id: `mcp-invocation-${secondInvocationId}`,
          type: 'mcp_tool_call',
          invocationId: secondInvocationId
        },
        { id: 'message-stream-3', type: 'message', content: '第三段。' }
      ]
    })
    terminalMessage.content = '第一段。第二段。第三段。'
    const runningMessage: ChatMessage = {
      ...terminalMessage,
      status: 'pending',
      agentRun: terminalMessage.agentRun
        ? {
            ...terminalMessage.agentRun,
            status: 'running',
            completedAt: undefined
          }
        : undefined
    }

    const screen = await render(
      <ChatMessageItem
        message={runningMessage}
        projectId="project-1"
        showTokenUsageDetails={false}
      />
    )

    const expectInterleavedOrder = () => {
      const narration = screen.container.querySelectorAll('.chat-agent-text')
      const activities = screen.container.querySelectorAll('.mcp-tool-activity')
      expect(narration).toHaveLength(3)
      expect(activities).toHaveLength(2)
      expect(
        narration[0].compareDocumentPosition(activities[0]) & Node.DOCUMENT_POSITION_FOLLOWING
      ).toBeTruthy()
      expect(
        activities[0].compareDocumentPosition(narration[1]) & Node.DOCUMENT_POSITION_FOLLOWING
      ).toBeTruthy()
      expect(
        narration[1].compareDocumentPosition(activities[1]) & Node.DOCUMENT_POSITION_FOLLOWING
      ).toBeTruthy()
      expect(
        activities[1].compareDocumentPosition(narration[2]) & Node.DOCUMENT_POSITION_FOLLOWING
      ).toBeTruthy()
    }

    expectInterleavedOrder()
    await screen.rerender(
      <ChatMessageItem
        message={terminalMessage}
        projectId="project-1"
        showTokenUsageDetails={false}
      />
    )
    expectInterleavedOrder()
    expect(screen.container.textContent?.match(/第一段。/g)).toHaveLength(1)
    expect(screen.container.textContent?.match(/第二段。/g)).toHaveLength(1)
    expect(screen.container.textContent?.match(/第三段。/g)).toHaveLength(1)
  })

  it('keeps the final image Artifact card visible after expanding the timeline', async () => {
    const hash = 'f'.repeat(64)
    const message = assistantMessage({
      toolCalls: [imageGenerationCall('image-1', '生成校园照片。')],
      toolResults: [imageGenerationResult('image-1', hash)],
      timeline: [
        { id: 'image-1', type: 'tool_call', callId: 'image-1' },
        { id: 'final-stream', type: 'message', content: '图片任务已完成。' }
      ]
    })
    message.content = '图片任务已完成。'

    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    expect(screen.container.textContent?.match(/图片任务已完成。/g)).toHaveLength(1)
    expect(screen.container.querySelectorAll('.image-generation-artifact-card')).toHaveLength(1)
    expect(screen.container.querySelector('.image-generation-activity__preview')).toBeNull()
    expect(screen.container.querySelector('.image-generation-artifact-preview')).not.toBeNull()
    expect(screen.container.textContent).toContain('生成校园照片。')
  })

  it('keeps the generated image preview under its Tool status until the run settles', async () => {
    const hash = 'e'.repeat(64)
    const message = assistantMessage({
      status: 'running',
      completedAt: undefined,
      toolCalls: [imageGenerationCall('image-running', '生成过程中的校园照片。')],
      toolResults: [imageGenerationResult('image-running', hash)],
      timeline: [{ id: 'image-running', type: 'tool_call', callId: 'image-running' }]
    })

    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    expect(screen.container.querySelector('.image-generation-activity__preview')).not.toBeNull()
    expect(screen.container.querySelector('.image-generation-artifact-section')).toBeNull()
  })

  it('renders one loaded-Skill activity and merges paged reads of one resource', async () => {
    const skill = {
      id: 'bundled:application:spreadsheets',
      name: 'Spreadsheets',
      revision: 'revision-1',
      source: { kind: 'bundled' as const, id: 'application:spreadsheets' }
    }
    const uri = `skill://package/${encodeURIComponent(skill.id)}/revision-1/references/workflows.md`
    const message = assistantMessage({
      activatedSkills: [skill, skill],
      toolCalls: [
        {
          id: 'read-1',
          tool: 'skills_read_resource',
          approvalStatus: 'not_required',
          reason: null,
          args: { uri, startByte: 0 }
        },
        {
          id: 'discover',
          tool: 'skills_list_resources',
          approvalStatus: 'not_required',
          reason: null,
          args: {}
        },
        {
          id: 'read-2',
          tool: 'skills_read_resource',
          approvalStatus: 'not_required',
          reason: null,
          args: { uri, startByte: 1024 }
        }
      ],
      toolResults: [
        { callId: 'read-1', tool: 'skills_read_resource', ok: true, result: { uri } },
        { callId: 'read-2', tool: 'skills_read_resource', ok: true, result: { uri } }
      ],
      timeline: [
        { id: 'read-1', type: 'tool_call', callId: 'read-1' },
        { id: 'blank', type: 'message', content: ' ' },
        { id: 'discover', type: 'tool_call', callId: 'discover' },
        { id: 'read-2', type: 'tool_call', callId: 'read-2' }
      ]
    })
    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    expect(screen.container.textContent?.match(/已加载工具/g)).toHaveLength(1)
    expect(screen.container.textContent).toContain('读取 电子表格 技能')
    expect(screen.container.textContent?.match(/已读取「电子表格」技能资料/g)).toHaveLength(1)
    expect(screen.container.textContent).not.toContain('skills_list_resources')
    expect(screen.container.textContent).not.toContain('skill://')
  })

  it('shows a native Office activity and a separate final file card', async () => {
    const message = assistantMessage({
      toolCalls: [
        {
          id: 'office-1',
          tool: 'office_spreadsheet',
          approvalStatus: 'approved',
          args: {
            request: { operation: 'add', filePath: 'reports/budget.xlsx' },
            reason: '在预算表中新增了季度合计，并保留原有格式。'
          },
          reason: '在预算表中新增了季度合计，并保留原有格式。'
        }
      ],
      toolResults: [
        {
          callId: 'office-1',
          tool: 'office_spreadsheet',
          ok: true,
          result: { exitCode: 0, timedOut: false, cancelled: false }
        }
      ],
      timeline: [{ id: 'office-1', type: 'tool_call', callId: 'office-1' }]
    })
    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    expect(screen.container.textContent?.match(/已编辑电子表格/g)).toHaveLength(1)
    expect(screen.container.textContent).toContain('在预算表中新增了季度合计，并保留原有格式。')
    expect(screen.container.textContent).toContain('budget.xlsx')
    expect(screen.container.textContent).toContain('电子表格 · XLSX')
    expect(screen.container.textContent).not.toContain('engineRevision')
    expect(screen.container.textContent).not.toContain('normalizedPath')

    await screen.getByRole('button', { name: '在文件夹中打开' }).click()
    expect(revealStoredProjectFile).toHaveBeenCalledWith('project-1', 'reports/budget.xlsx')
  })

  it('aggregates adjacent Office calls by document kind, file identity and operation mode', async () => {
    const calls = [
      {
        id: 'edit-1',
        tool: 'office_document',
        approvalStatus: 'approved' as const,
        args: { operation: 'add', path: './report.docx' },
        reason: '新增了执行摘要。'
      },
      {
        id: 'edit-2',
        tool: 'office_document',
        approvalStatus: 'approved' as const,
        args: { operation: 'set', documentPath: 'report.docx' },
        reason: '更新了项目结论。'
      },
      {
        id: 'view-1',
        tool: 'office_document',
        approvalStatus: 'not_required' as const,
        args: { operation: 'get', path: 'report.docx' },
        reason: '检查了标题层级。'
      },
      {
        id: 'view-2',
        tool: 'office_document',
        approvalStatus: 'not_required' as const,
        args: { operation: 'view', documentPath: './report.docx' },
        reason: '检查了页面布局。'
      },
      {
        id: 'other-file',
        tool: 'office_document',
        approvalStatus: 'approved' as const,
        args: { operation: 'add', path: 'appendix.docx' },
        reason: '新增了附录。'
      }
    ]
    const message = assistantMessage({
      toolCalls: calls,
      toolResults: calls.map((call) => ({
        callId: call.id,
        tool: call.tool,
        ok: true,
        result: { exitCode: 0 }
      })),
      timeline: [
        { id: 'edit-1', type: 'tool_call', callId: 'edit-1' },
        { id: 'blank', type: 'message', content: ' ' },
        { id: 'edit-2', type: 'tool_call', callId: 'edit-2' },
        { id: 'view-1', type: 'tool_call', callId: 'view-1' },
        { id: 'view-2', type: 'tool_call', callId: 'view-2' },
        { id: 'other-file', type: 'tool_call', callId: 'other-file' }
      ]
    })
    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    expect(screen.container.textContent).toContain('已编辑文档 · 2 项操作')
    expect(screen.container.textContent).toContain('已查看文档信息 · 2 项检查')
    expect(screen.container.textContent?.match(/已编辑文档/g)).toHaveLength(2)
    expect(screen.container.textContent).toContain('新增了执行摘要。')
    expect(screen.container.textContent).toContain('更新了项目结论。')
    expect(screen.container.textContent).toContain('检查了标题层级。')
    expect(screen.container.textContent).toContain('检查了页面布局。')
  })

  it('renders adjacent mixed-result Office help probes as one document check', async () => {
    const calls = Array.from({ length: 9 }, (_, index) => ({
      id: `help-${index + 1}`,
      tool: 'office_document' as const,
      approvalStatus: 'not_required' as const,
      args: {
        request:
          index === 0
            ? { operation: 'help', verb: 'create', element: 'document' }
            : { operation: 'help', verb: 'add', element: `element-${index + 1}` }
      },
      reason: `检查第 ${index + 1} 项文档能力。`
    }))
    const message = assistantMessage({
      toolCalls: calls,
      toolResults: calls.map((call, index) =>
        index === 0
          ? {
              callId: call.id,
              tool: call.tool,
              ok: false,
              error: 'Office help probe failed.'
            }
          : {
              callId: call.id,
              tool: call.tool,
              ok: true,
              result: { exitCode: 0 }
            }
      ),
      timeline: calls.map((call) => ({
        id: call.id,
        type: 'tool_call' as const,
        callId: call.id
      }))
    })
    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    expect(screen.container.querySelectorAll('.agent-activity--office')).toHaveLength(1)
    expect(screen.container.textContent).toContain('文档检查 · 成功 8 项 · 失败 1 项')
    expect(screen.container.querySelectorAll('.office-activity__item')).toHaveLength(9)
    expect(
      screen.container.querySelectorAll('.office-activity__item[data-status="completed"]')
    ).toHaveLength(8)
    expect(
      screen.container.querySelectorAll('.office-activity__item[data-status="failed"]')
    ).toHaveLength(1)
    expect(screen.container.textContent).toContain('Office help probe failed.')
  })

  it('reports mixed Office outcomes accurately and keeps every operation inspectable', async () => {
    const calls = [
      {
        id: 'edit-failed',
        tool: 'office_document',
        approvalStatus: 'approved' as const,
        args: { operation: 'set', path: 'report.docx' },
        reason: '替换文档中的指定文字。'
      },
      ...['添加内容概述。', '添加主要内容。', '添加性质说明。'].map((reason, index) => ({
        id: `edit-success-${index + 1}`,
        tool: 'office_document',
        approvalStatus: 'approved' as const,
        args: { operation: 'add', path: 'report.docx' },
        reason
      }))
    ]
    const message = assistantMessage({
      toolCalls: calls,
      toolResults: calls.map((call, index) =>
        index === 0
          ? {
              callId: call.id,
              tool: call.tool,
              ok: false,
              error: 'Office replacement find cannot be empty.'
            }
          : {
              callId: call.id,
              tool: call.tool,
              ok: true,
              result: { exitCode: 0 }
            }
      ),
      timeline: calls.map((call) => ({
        id: call.id,
        type: 'tool_call' as const,
        callId: call.id
      }))
    })
    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    expect(screen.container.textContent).toContain('文档操作 · 成功 3 项 · 失败 1 项')
    expect(screen.container.querySelectorAll('.office-activity__item')).toHaveLength(4)
    expect(
      screen.container.querySelectorAll('.office-activity__item[data-status="completed"]')
    ).toHaveLength(3)
    expect(
      screen.container.querySelectorAll('.office-activity__item[data-status="failed"]')
    ).toHaveLength(1)
    expect(screen.container.textContent).toContain('替换文档中的指定文字。')
    expect(screen.container.textContent).toContain('Office replacement find cannot be empty.')
    expect(screen.container.textContent).toContain('添加内容概述。')
    expect(screen.container.textContent).not.toContain('文档处理失败 · 4 项操作')
  })

  it('renders multiple Office outputs as rows in one connected card with Office icons', async () => {
    const paths = ['图片描述_副本1.docx', '图片描述_副本2.docx', '图片描述_副本3.docx']
    const calls = paths.map((path, index) => ({
      id: `create-${index + 1}`,
      tool: 'office_document',
      approvalStatus: 'approved' as const,
      args: { operation: 'create', path },
      reason: `创建第 ${index + 1} 份文档。`
    }))
    const message = assistantMessage({
      toolCalls: calls,
      toolResults: calls.map((call) => ({
        callId: call.id,
        tool: call.tool,
        ok: true,
        result: { exitCode: 0 }
      })),
      timeline: calls.map((call) => ({
        id: call.id,
        type: 'tool_call' as const,
        callId: call.id
      }))
    })
    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    expect(screen.container.querySelectorAll('.office-artifact-list')).toHaveLength(1)
    const rows = screen.container.querySelectorAll('.office-artifact-card')
    expect(rows).toHaveLength(3)
    expect(rows[0]?.parentElement).toBe(rows[1]?.parentElement)
    expect(rows[1]?.parentElement).toBe(rows[2]?.parentElement)
    rows.forEach((row) => {
      expect(row.querySelector('.office-artifact-card__icon img')).not.toBeNull()
      expect(row.querySelector('.office-artifact-card__icon svg')).toBeNull()
    })
    paths.forEach((path) => expect(screen.container.textContent).toContain(path))
    expect(screen.getByRole('button', { name: '在文件夹中打开' }).elements()).toHaveLength(3)
  })

  it('places generated image artifacts above Office file cards', async () => {
    const hash = 'b'.repeat(64)
    const message = assistantMessage({
      toolCalls: [
        {
          id: 'office-1',
          tool: 'office_spreadsheet',
          approvalStatus: 'approved',
          args: { operation: 'create', path: '浙江大学_工科专业排名.xlsx' },
          reason: '创建高校专业排名电子表格。'
        },
        {
          id: 'image-1',
          tool: 'image_generation',
          approvalStatus: 'not_required',
          args: {
            request: { operation: 'generate', hasInputImage: false },
            reason: '生成校园风景图片。'
          },
          reason: '生成校园风景图片。'
        }
      ],
      toolResults: [
        {
          callId: 'office-1',
          tool: 'office_spreadsheet',
          ok: true,
          result: { exitCode: 0 }
        },
        {
          callId: 'image-1',
          tool: 'image_generation',
          ok: true,
          result: {
            schemaVersion: 1,
            status: 'succeeded',
            operation: 'generate',
            artifact: {
              artifactId: `sha256:${hash}`,
              uri: `image-artifact://sha256/${hash}`,
              kind: 'image',
              format: 'jpeg',
              mimeType: 'image/jpeg',
              width: 1536,
              height: 1024,
              sizeBytes: 4096,
              sha256: hash
            },
            audit: {
              executionId: 'execution-1',
              requestFingerprint: `sha256:${hash}`,
              providerProfileId: 'profile-1',
              adapterId: 'smartmlSeedream',
              profileRevision: 1,
              modelId: 'image-model',
              createdAt: 100,
              completedAt: 120,
              durationMs: 20
            }
          }
        }
      ],
      timeline: [
        { id: 'office-1', type: 'tool_call', callId: 'office-1' },
        { id: 'image-1', type: 'tool_call', callId: 'image-1' }
      ]
    })
    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    const imageArtifacts = screen.container.querySelector('.image-generation-artifact-section')
    const officeArtifacts = screen.container.querySelector('.office-artifact-list')

    expect(screen.container.querySelector('.image-generation-activity__preview')).toBeNull()
    expect(imageArtifacts).not.toBeNull()
    expect(officeArtifacts).not.toBeNull()
    if (!imageArtifacts || !officeArtifacts) {
      throw new Error('Expected both final image and Office artifact sections.')
    }
    expect(
      imageArtifacts.compareDocumentPosition(officeArtifacts) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
  })

  it('restores the settled file card from the original run_command Session receipt', async () => {
    const hash = 'c'.repeat(64)
    const pdfBytes = Uint8Array.from([0x25, 0x50, 0x44, 0x46])
    readArtifact.mockResolvedValueOnce({
      ok: true,
      value: {
        schemaVersion: 1,
        artifact: {
          artifactId: `sha256:${hash}`,
          uri: `artifact://sha256/${hash}`,
          kind: 'document',
          format: 'pdf',
          mimeType: 'application/pdf',
          sizeBytes: 4096,
          sha256: hash
        },
        fileName: `artifact-${hash.slice(0, 12)}.pdf`,
        bytes: pdfBytes
      }
    })
    const createObjectUrl = vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:managed-pdf')
    const revokeObjectUrl = vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => undefined)
    const message = assistantMessage({
      toolCalls: [
        {
          id: 'session-pdf',
          tool: 'run_command',
          approvalStatus: 'not_required',
          reason: null,
          args: { command: 'render-pdf' }
        }
      ],
      toolResults: [],
      commandSessions: {
        'session-pdf': {
          callId: 'session-pdf',
          status: 'exited',
          latestSequence: 0,
          outputTruncated: false,
          outputs: [
            {
              name: 'reports/final.pdf',
              kind: 'document',
              readPath: `artifact://sha256/${hash}`,
              mimeType: 'application/pdf',
              sizeBytes: 4096,
              sha256: hash
            }
          ]
        }
      },
      timeline: [{ id: 'session-pdf', type: 'tool_call', callId: 'session-pdf' }]
    })
    const screen = await render(
      <ChatMessageItem
        conversationId="conversation-1"
        message={message}
        projectId="project-1"
        showTokenUsageDetails={false}
      />
    )

    await expect.element(screen.getByText('reports/final.pdf')).toBeVisible()
    expect(screen.container.textContent).not.toContain(`artifact://sha256/${hash}`)
    await expect.element(screen.getByRole('button', { name: '下载', exact: true })).toBeVisible()
    await screen.getByRole('button', { name: '下载', exact: true }).click()
    await expect.poll(() => readArtifact.mock.calls.length).toBe(1)
    expect(readArtifact).toHaveBeenCalledWith({
      schemaVersion: 1,
      conversationId: 'conversation-1',
      artifact: expect.objectContaining({
        artifactId: `sha256:${hash}`,
        uri: `artifact://sha256/${hash}`,
        kind: 'document'
      })
    })
    expect(createObjectUrl).toHaveBeenCalledTimes(1)
    expect(revealStoredProjectFile).not.toHaveBeenCalledWith(
      'project-1',
      `artifact://sha256/${hash}`
    )
    createObjectUrl.mockRestore()
    revokeObjectUrl.mockRestore()
  })

  it('renders an edited presentation card after the answer when its background Session settles', async () => {
    const callId = 'edit-presentation'
    const path = '南京大学介绍_已编辑.pptx'
    const message = assistantMessage({
      toolCalls: [
        {
          id: callId,
          tool: 'run_command',
          approvalStatus: 'approved',
          reason: '编辑现有演示文稿。',
          args: { command: 'node scripts/edit_deck.mjs --output 南京大学介绍_已编辑.pptx' }
        }
      ],
      toolResults: [
        {
          callId,
          tool: 'run_command',
          ok: true,
          result: {
            status: 'running',
            sessionId: 'cmd_1234567890abcdef1234567890abcdef'
          }
        }
      ],
      commandSessions: {
        [callId]: {
          callId,
          sessionId: 'cmd_1234567890abcdef1234567890abcdef',
          status: 'exited',
          startedAt: 10,
          endedAt: 20,
          exitCode: 0,
          latestSequence: 1,
          outputTruncated: false,
          artifactObservation: presentationArtifactObservation(path)
        }
      },
      timeline: [{ id: callId, type: 'tool_call', callId }]
    })
    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    await expect.element(screen.getByText(path, { exact: true })).toBeVisible()
    const answer = screen.container.querySelector('.chat-agent-text')
    const artifactList = screen.container.querySelector('.office-artifact-list')
    expect(answer).not.toBeNull()
    expect(artifactList).not.toBeNull()
    if (!answer || !artifactList) {
      throw new Error('Expected both the final answer and edited presentation card.')
    }
    expect(
      answer.compareDocumentPosition(artifactList) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
    expect(screen.container.querySelectorAll('.office-artifact-card')).toHaveLength(1)
  })

  it('does not render a success file card for a failed observed command', async () => {
    const message = assistantMessage({
      toolCalls: [
        {
          id: 'command-1',
          tool: 'run_command',
          approvalStatus: 'approved',
          reason: null,
          args: { command: 'python build.py' }
        }
      ],
      toolResults: [
        {
          callId: 'command-1',
          tool: 'run_command',
          ok: false,
          error: 'Command failed',
          result: {
            exitCode: 1,
            artifactObservation: {
              status: 'complete',
              changes: [
                {
                  kind: 'created',
                  artifactKind: 'spreadsheet',
                  path: 'partial.xlsx',
                  scope: 'workspace',
                  after: { validation: { status: 'valid' } }
                }
              ]
            }
          }
        }
      ],
      timeline: [{ id: 'command-1', type: 'tool_call', callId: 'command-1' }]
    })
    const screen = await render(
      <ChatMessageItem message={message} projectId="project-1" showTokenUsageDetails={false} />
    )

    expect(screen.container.querySelector('.office-artifact-card')).toBeNull()
  })
})
