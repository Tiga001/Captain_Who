import type { ActivatedSkillSummary, AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import type { ChatAgentRunView } from '../chatTypes'
import { groupTimelineItems } from '../components/chatMessageItemUtils'
import {
  getActivatedSkills,
  getOfficeActivityGroupIdentity,
  getOfficeActivityView,
  getOfficeArtifactEntries,
  getSkillResourceActivityItem,
  getToolActivityStatus,
  isHiddenSkillTool,
  parseSkillResourceUri
} from '../skillOfficeActivity'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

const spreadsheetSkill: ActivatedSkillSummary = {
  id: 'bundled:application:spreadsheets',
  name: 'Spreadsheets',
  revision: 'revision-1',
  source: { kind: 'bundled', id: 'application:spreadsheets' }
}

function toolCall(
  overrides: Partial<AgentToolCall> & Pick<AgentToolCall, 'id' | 'tool'>
): AgentToolCall {
  return {
    args: {},
    approvalStatus: 'not_required',
    reason: null,
    ...overrides
  }
}

function toolResult(
  overrides: Partial<AgentToolResult> & Pick<AgentToolResult, 'callId' | 'tool'>
): AgentToolResult {
  return {
    ok: true,
    ...overrides
  }
}

function run(overrides: Partial<ChatAgentRunView> = {}): ChatAgentRunView {
  return {
    runId: 'run-1',
    status: 'completed',
    completedAt: 10,
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    diffs: [],
    timeline: [],
    ...overrides
  }
}

describe('Skill and Office activity derivation', () => {
  it('uses activatedSkills as the sole loaded-Skill inventory and deduplicates ids', () => {
    const currentRun = run({
      activatedSkills: [spreadsheetSkill, { ...spreadsheetSkill, name: 'Duplicate' }]
    })

    expect(getActivatedSkills(currentRun)).toEqual([spreadsheetSkill])
  })

  it('hides the activation transport after the authoritative loaded-Skill activity appears', () => {
    expect(isHiddenSkillTool('skills_activate')).toBe(true)
  })

  it('parses a Skill URI only when its decoded id belongs to the activated run', () => {
    const uri = `skill://package/${encodeURIComponent(spreadsheetSkill.id)}/revision-1/references/workflows.md`

    expect(parseSkillResourceUri(uri, [spreadsheetSkill])).toEqual({
      resourcePath: 'references/workflows.md',
      skill: spreadsheetSkill
    })
    expect(parseSkillResourceUri(uri, [])).toBeUndefined()
  })

  it('gives progressive pages of the same Skill resource one stable grouping key', () => {
    const uri = `skill://package/${encodeURIComponent(spreadsheetSkill.id)}/revision-1/references/workflows.md`
    const calls = [
      toolCall({ id: 'read-page-1', tool: 'skills_read_resource', args: { uri, startByte: 0 } }),
      toolCall({ id: 'discover', tool: 'skills_list_resources', args: {} }),
      toolCall({ id: 'read-page-2', tool: 'skills_read_resource', args: { uri, startByte: 1024 } })
    ]
    const results = [
      toolResult({ callId: 'read-page-1', tool: 'skills_read_resource', result: { uri } }),
      toolResult({ callId: 'read-page-2', tool: 'skills_read_resource', result: { uri } })
    ]
    const currentRun = run({
      activatedSkills: [spreadsheetSkill],
      toolCalls: calls,
      toolResults: results
    })
    const items = [calls[0], calls[2]]
      .map((call) => getSkillResourceActivityItem(currentRun, call))
      .filter((item) => item !== undefined)

    expect(new Set(items.map((item) => item.resourceKey)).size).toBe(1)
  })

  it('classifies templates only from the normalized templates path or sourcePrefix', () => {
    const root = `skill://package/${encodeURIComponent(spreadsheetSkill.id)}/revision-1`
    const templateCall = toolCall({
      id: 'template',
      tool: 'skills_materialize_resource',
      args: { sourceUri: root, sourcePrefix: 'templates/monthly', destination: 'report' }
    })
    const otherCall = toolCall({
      id: 'other',
      tool: 'skills_materialize_resource',
      args: { sourceUri: `${root}/assets/logo.png`, destination: 'logo.png' }
    })
    const currentRun = run({ activatedSkills: [spreadsheetSkill] })

    expect(getSkillResourceActivityItem(currentRun, templateCall)?.kind).toBe('template')
    expect(getSkillResourceActivityItem(currentRun, otherCall)?.kind).toBe('capability')
  })

  it('prioritizes structured rejection, cancellation and timeout over result.ok', () => {
    const call = toolCall({ id: 'office-1', tool: 'office_spreadsheet' })
    expect(
      getToolActivityStatus(
        call,
        toolResult({ callId: call.id, tool: call.tool, result: { status: 'rejected' } })
      )
    ).toBe('rejected')
    expect(
      getToolActivityStatus(
        call,
        toolResult({ callId: call.id, tool: call.tool, result: { cancelled: true } })
      )
    ).toBe('cancelled')
    expect(
      getToolActivityStatus(
        call,
        toolResult({ callId: call.id, tool: call.tool, result: { timedOut: true } })
      )
    ).toBe('failed')
  })

  it('treats an approval rejection without a ToolResult as rejected', () => {
    const call = toolCall({
      id: 'office-rejected',
      tool: 'office_document',
      approvalStatus: 'rejected'
    })

    expect(getToolActivityStatus(call, undefined)).toBe('rejected')
  })

  it('keeps the user-facing reason and execution error separate for failed Office calls', () => {
    const call = toolCall({
      id: 'office-failed',
      tool: 'office_document',
      approvalStatus: 'approved',
      args: { operation: 'set', path: 'report.docx' },
      reason: '替换文档中的指定文字。'
    })
    const currentRun = run({
      toolResults: [
        toolResult({
          callId: call.id,
          tool: call.tool,
          ok: false,
          error: 'Office replacement find cannot be empty.'
        })
      ]
    })

    expect(getOfficeActivityView(currentRun, call)).toMatchObject({
      detail: '替换文档中的指定文字。',
      error: 'Office replacement find cannot be empty.',
      reason: '替换文档中的指定文字。',
      status: 'failed'
    })
  })

  it('keeps one Office card moving from approval to execution to the paired result', () => {
    const waitingCall = toolCall({
      id: 'office-1',
      tool: 'office_spreadsheet',
      approvalStatus: 'required',
      args: { operation: 'add', path: 'budget.xlsx' },
      reason: '新增季度合计并保留格式。'
    })
    expect(getOfficeActivityView(run(), waitingCall)?.status).toBe('waiting')

    const approvedCall = { ...waitingCall, approvalStatus: 'approved' as const }
    expect(getOfficeActivityView(run(), approvedCall)?.status).toBe('running')

    const completedRun = run({
      toolResults: [
        toolResult({
          callId: waitingCall.id,
          tool: waitingCall.tool,
          result: { exitCode: 0, timedOut: false, cancelled: false }
        })
      ]
    })
    expect(getOfficeActivityView(completedRun, approvedCall)).toMatchObject({
      detail: '新增季度合计并保留格式。',
      mode: 'edit',
      status: 'completed'
    })
  })

  it('groups Office reads by document kind and read category while retaining mutation targets', () => {
    const firstEdit = toolCall({
      id: 'edit-1',
      tool: 'office_spreadsheet',
      args: { request: { operation: 'add', filePath: './budget.xlsx' }, reason: 'Add totals' }
    })
    const secondEdit = toolCall({
      id: 'edit-2',
      tool: 'office_spreadsheet',
      args: { request: { operation: 'set', filePath: 'budget.xlsx' }, reason: 'Update totals' }
    })
    const view = toolCall({
      id: 'view-1',
      tool: 'office_spreadsheet',
      args: { operation: 'get', path: 'budget.xlsx' }
    })
    const otherFile = toolCall({
      id: 'edit-3',
      tool: 'office_spreadsheet',
      args: { operation: 'add', path: 'forecast.xlsx' }
    })
    const readCalls = [
      toolCall({ id: 'help', tool: 'office_spreadsheet', args: { operation: 'help' } }),
      toolCall({ id: 'status', tool: 'office_spreadsheet', args: { operation: 'status' } }),
      view,
      toolCall({
        id: 'query',
        tool: 'office_spreadsheet',
        args: { operation: 'query', path: 'forecast.xlsx' }
      }),
      toolCall({
        id: 'view',
        tool: 'office_spreadsheet',
        args: { operation: 'view', path: 'archive.xlsx' }
      }),
      toolCall({
        id: 'validate',
        tool: 'office_spreadsheet',
        args: { operation: 'validate', path: 'other.xlsx' }
      })
    ]
    const presentationRead = toolCall({
      id: 'presentation-help',
      tool: 'office_presentation',
      args: { operation: 'help' }
    })
    const firstExport = toolCall({
      id: 'export-1',
      tool: 'office_spreadsheet',
      args: { operation: 'view', path: 'budget.xlsx', outputPath: 'preview-1.png' }
    })
    const secondExport = toolCall({
      id: 'export-2',
      tool: 'office_spreadsheet',
      args: { operation: 'view', path: 'budget.xlsx', outputPath: 'preview-2.png' }
    })

    expect(getOfficeActivityGroupIdentity(firstEdit)?.key).toBe(
      getOfficeActivityGroupIdentity(secondEdit)?.key
    )
    expect(getOfficeActivityGroupIdentity(firstEdit)?.key).not.toBe(
      getOfficeActivityGroupIdentity(view)?.key
    )
    expect(getOfficeActivityGroupIdentity(firstEdit)?.key).not.toBe(
      getOfficeActivityGroupIdentity(otherFile)?.key
    )
    expect(new Set(readCalls.map((call) => getOfficeActivityGroupIdentity(call)?.key))).toEqual(
      new Set([getOfficeActivityGroupIdentity(view)?.key])
    )
    expect(getOfficeActivityGroupIdentity(view)).toMatchObject({
      category: 'read',
      fileIdentity: 'read-check'
    })
    expect(getOfficeActivityGroupIdentity(view)?.key).not.toBe(
      getOfficeActivityGroupIdentity(presentationRead)?.key
    )
    expect(getOfficeActivityGroupIdentity(firstExport)?.key).not.toBe(
      getOfficeActivityGroupIdentity(secondExport)?.key
    )
    expect(getOfficeActivityView(run(), readCalls[1])).toMatchObject({
      category: 'read',
      mode: 'view',
      operation: 'status'
    })
  })

  it('groups only consecutive Office read checks and preserves narration and tool boundaries', () => {
    const readCalls = [
      toolCall({ id: 'help', tool: 'office_document', args: { operation: 'help' } }),
      toolCall({ id: 'status', tool: 'office_document', args: { operation: 'status' } }),
      toolCall({
        id: 'get',
        tool: 'office_document',
        args: { operation: 'get', path: 'report.docx' }
      }),
      toolCall({
        id: 'query',
        tool: 'office_document',
        args: { operation: 'query', path: 'appendix.docx' }
      }),
      toolCall({
        id: 'view',
        tool: 'office_document',
        args: { operation: 'view', path: 'other.docx' }
      }),
      toolCall({
        id: 'validate',
        tool: 'office_document',
        args: { operation: 'validate', path: 'checked.docx' }
      })
    ]
    const afterNarration = toolCall({
      id: 'status-after-narration',
      tool: 'office_document',
      args: { operation: 'status' }
    })
    const command = toolCall({ id: 'command', tool: 'run_command' })
    const afterTool = toolCall({
      id: 'get-after-tool',
      tool: 'office_document',
      args: { operation: 'get', path: 'report.docx' }
    })
    const currentRun = run({
      toolCalls: [...readCalls, afterNarration, command, afterTool],
      timeline: [
        ...readCalls.map((call) => ({
          id: call.id,
          type: 'tool_call' as const,
          callId: call.id
        })),
        { id: 'narration', type: 'message', content: '继续核对渲染状态。' },
        {
          id: afterNarration.id,
          type: 'tool_call',
          callId: afterNarration.id
        },
        { id: command.id, type: 'tool_call', callId: command.id },
        { id: afterTool.id, type: 'tool_call', callId: afterTool.id }
      ]
    })

    const grouped = groupTimelineItems(currentRun, currentRun.timeline)
    const officeGroups = grouped.filter((item) => item.type === 'office_group')

    expect(officeGroups.map((item) => item.callIds)).toEqual([
      readCalls.map((call) => call.id),
      [afterNarration.id],
      [afterTool.id]
    ])
    expect(grouped.map((item) => item.type)).toEqual([
      'office_group',
      'message',
      'office_group',
      'run_command_group',
      'office_group'
    ])
  })

  it('keeps nine adjacent help outcomes in one group but separates write targets', () => {
    const helpCalls = Array.from({ length: 9 }, (_, index) =>
      toolCall({
        id: `help-${index + 1}`,
        tool: 'office_document',
        args: {
          request:
            index === 0
              ? { operation: 'help', verb: 'create', element: 'document' }
              : { operation: 'help', verb: 'add', element: `element-${index + 1}` }
        },
        reason: `检查第 ${index + 1} 项文档能力。`
      })
    )
    const writes = [
      toolCall({
        id: 'create-a',
        tool: 'office_document',
        args: { operation: 'create', path: 'a.docx' }
      }),
      toolCall({
        id: 'create-b',
        tool: 'office_document',
        args: { operation: 'create', path: 'b.docx' }
      }),
      toolCall({
        id: 'edit-a',
        tool: 'office_document',
        args: { operation: 'set', path: 'a.docx' }
      }),
      toolCall({
        id: 'edit-b',
        tool: 'office_document',
        args: { operation: 'set', path: 'b.docx' }
      }),
      toolCall({
        id: 'export-a',
        tool: 'office_document',
        args: { operation: 'view', path: 'a.docx', outputPath: 'a.png' }
      }),
      toolCall({
        id: 'export-b',
        tool: 'office_document',
        args: { operation: 'view', path: 'a.docx', outputPath: 'b.png' }
      })
    ]
    const calls = [...helpCalls, ...writes]
    const currentRun = run({
      toolCalls: calls,
      toolResults: helpCalls.map((call, index) =>
        index === 0
          ? toolResult({
              callId: call.id,
              tool: call.tool,
              ok: false,
              error: 'Office help probe failed.'
            })
          : toolResult({ callId: call.id, tool: call.tool, result: { exitCode: 0 } })
      ),
      timeline: calls.map((call) => ({
        id: call.id,
        type: 'tool_call' as const,
        callId: call.id
      }))
    })

    const officeGroups = groupTimelineItems(currentRun, currentRun.timeline).filter(
      (item) => item.type === 'office_group'
    )

    expect(officeGroups[0]?.callIds).toEqual(helpCalls.map((call) => call.id))
    expect(officeGroups).toHaveLength(7)
    expect(officeGroups.slice(1).map((item) => item.callIds)).toEqual(
      writes.map((call) => [call.id])
    )
  })

  it('creates validated Office artifact cards and deduplicates the final path', () => {
    const command = toolCall({ id: 'command-1', tool: 'run_command' })
    const observation = {
      status: 'complete',
      changes: [
        {
          kind: 'created',
          artifactKind: 'spreadsheet',
          path: 'reports/budget.xlsx',
          scope: 'workspace',
          after: { validation: { status: 'valid' } }
        },
        {
          kind: 'modified',
          artifactKind: 'spreadsheet',
          path: 'reports/budget.xlsx',
          scope: 'workspace',
          after: { validation: { status: 'valid' } }
        }
      ]
    }
    const currentRun = run({
      toolCalls: [command],
      toolResults: [
        toolResult({
          callId: command.id,
          tool: command.tool,
          result: { exitCode: 0, artifactObservation: observation }
        })
      ]
    })

    expect(getOfficeArtifactEntries(currentRun)).toEqual([
      {
        artifactKind: 'spreadsheet',
        changeKind: 'modified',
        id: 'workspace:reports/budget.xlsx',
        path: 'reports/budget.xlsx',
        scope: 'workspace'
      }
    ])
  })

  it('retains failed observations in agentRun but never turns them into success cards', () => {
    const command = toolCall({ id: 'command-1', tool: 'run_command' })
    const currentRun = run({
      toolCalls: [command],
      toolResults: [
        toolResult({
          callId: command.id,
          tool: command.tool,
          ok: false,
          result: {
            exitCode: 1,
            execution: {
              artifactObservation: {
                status: 'complete',
                changes: [
                  {
                    kind: 'created',
                    artifactKind: 'document',
                    path: 'partial.docx',
                    scope: 'workspace',
                    after: { validation: { status: 'valid' } }
                  }
                ]
              }
            }
          }
        })
      ]
    })

    expect(currentRun.toolResults[0].result).toBeDefined()
    expect(getOfficeArtifactEntries(currentRun)).toEqual([])
  })

  it('accepts CSV not_applicable validation but rejects unchecked Office files', () => {
    const command = toolCall({ id: 'command-1', tool: 'run_command' })
    const currentRun = run({
      toolCalls: [command],
      toolResults: [
        toolResult({
          callId: command.id,
          tool: command.tool,
          result: {
            exitCode: 0,
            artifactObservation: {
              status: 'partial',
              changes: [
                {
                  kind: 'created',
                  artifactKind: 'spreadsheet',
                  path: 'data.csv',
                  scope: 'workspace',
                  after: { validation: { status: 'not_applicable' } }
                },
                {
                  kind: 'created',
                  artifactKind: 'document',
                  path: 'unchecked.docx',
                  scope: 'workspace',
                  after: { validation: { status: 'unchecked' } }
                }
              ]
            }
          }
        })
      ]
    })

    expect(getOfficeArtifactEntries(currentRun).map((entry) => entry.path)).toEqual(['data.csv'])
  })

  it('classifies a validated expected output without scope and rejects observations from rejected commands', () => {
    const expectedCommand = toolCall({ id: 'expected', tool: 'run_command' })
    const rejectedCommand = toolCall({ id: 'rejected', tool: 'run_command' })
    const currentRun = run({
      toolCalls: [expectedCommand, rejectedCommand],
      toolResults: [
        toolResult({
          callId: expectedCommand.id,
          tool: expectedCommand.tool,
          result: {
            exitCode: 0,
            artifactObservation: {
              status: 'complete',
              changes: [],
              expectedOutputs: [
                {
                  requestedPath: '@desktop/report.xlsx',
                  outcome: 'created',
                  path: '@desktop/report.xlsx',
                  artifactKind: 'spreadsheet',
                  metadata: { validation: { status: 'valid' } }
                }
              ]
            }
          }
        }),
        toolResult({
          callId: rejectedCommand.id,
          tool: rejectedCommand.tool,
          result: {
            status: 'rejected',
            artifactObservation: {
              status: 'complete',
              changes: [
                {
                  kind: 'created',
                  artifactKind: 'document',
                  path: 'must-not-show.docx',
                  scope: 'workspace',
                  after: { validation: { status: 'valid' } }
                }
              ]
            }
          }
        })
      ]
    })

    expect(getOfficeArtifactEntries(currentRun)).toEqual([
      {
        artifactKind: 'spreadsheet',
        changeKind: 'created',
        id: 'external:@desktop/report.xlsx',
        path: '@desktop/report.xlsx',
        scope: 'external'
      }
    ])
  })

  it('adapts a managed PDF receipt to the existing settled file card projection', () => {
    const hash = 'b'.repeat(64)
    const command = toolCall({ id: 'command-pdf', tool: 'command_session' })
    const currentRun = run({
      toolCalls: [command],
      toolResults: [
        toolResult({
          callId: command.id,
          tool: command.tool,
          result: {
            status: 'completed',
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
        })
      ]
    })

    expect(getOfficeArtifactEntries(currentRun)).toEqual([
      {
        artifactKind: 'document',
        changeKind: 'created',
        displayName: 'reports/final.pdf',
        id: `managed:artifact://sha256/${hash}`,
        managedArtifact: {
          artifactId: `sha256:${hash}`,
          format: 'pdf',
          kind: 'document',
          mimeType: 'application/pdf',
          sha256: hash,
          sizeBytes: 4096,
          uri: `artifact://sha256/${hash}`
        },
        managedReadPath: `artifact://sha256/${hash}`,
        path: `artifact://sha256/${hash}`,
        scope: 'external'
      }
    ])
  })

  it('restores a managed PDF card from the original run_command Session snapshot', () => {
    const hash = 'c'.repeat(64)
    const command = toolCall({ id: 'command-pdf-restart', tool: 'run_command' })
    const readPath = `artifact://sha256/${hash}`
    const restoredRun = run({
      toolCalls: [command],
      toolResults: [],
      commandSessions: {
        [command.id]: {
          callId: command.id,
          status: 'exited',
          latestSequence: 0,
          outputTruncated: false,
          outputs: [
            {
              name: 'reports/restored.pdf',
              kind: 'document',
              readPath,
              mimeType: 'application/pdf',
              sizeBytes: 8192,
              sha256: hash
            }
          ]
        }
      }
    })

    expect(getOfficeArtifactEntries(restoredRun)).toEqual([
      {
        artifactKind: 'document',
        changeKind: 'created',
        displayName: 'reports/restored.pdf',
        id: `managed:${readPath}`,
        managedArtifact: {
          artifactId: `sha256:${hash}`,
          format: 'pdf',
          kind: 'document',
          mimeType: 'application/pdf',
          sha256: hash,
          sizeBytes: 8192,
          uri: readPath
        },
        managedReadPath: readPath,
        path: readPath,
        scope: 'external'
      }
    ])
  })
})
