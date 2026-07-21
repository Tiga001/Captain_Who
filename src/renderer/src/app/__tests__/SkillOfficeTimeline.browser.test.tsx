// Renderer browser regressions for persisted Skill/Office timeline activity and Office files.

import type { ChatMessage } from '../../features/chat/chatTypes'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ChatMessageItem } from '../../features/chat/components/ChatMessageItem'

const { revealStoredProjectFile } = vi.hoisted(() => ({
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
  'agent.office.edited': '已编辑{kind}',
  'agent.office.viewed': '已查看{kind}信息',
  'agent.office.groupOperations': '{label} · {count} 项操作',
  'agent.office.groupChecks': '{label} · {count} 项检查',
  'agent.office.files': 'Office 文件',
  'agent.office.reveal': '在文件夹中打开',
  'agent.office.revealUnavailable': '无法定位',
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
  hostClient: {}
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
      diffs: [],
      timeline: [],
      ...overrides
    }
  }
}

describe('Skill and Office chat timeline', () => {
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
          args: { uri, startByte: 0 }
        },
        {
          id: 'discover',
          tool: 'skills_list_resources',
          approvalStatus: 'not_required',
          args: {}
        },
        {
          id: 'read-2',
          tool: 'skills_read_resource',
          approvalStatus: 'not_required',
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

  it('does not render a success file card for a failed observed command', async () => {
    const message = assistantMessage({
      toolCalls: [
        {
          id: 'command-1',
          tool: 'run_command',
          approvalStatus: 'approved',
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
