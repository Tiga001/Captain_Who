import type { ActivatedSkillSummary, AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import type { ChatAgentRunView } from '../chatTypes'
import {
  getActivatedSkills,
  getOfficeActivityGroupIdentity,
  getOfficeActivityView,
  getOfficeArtifactEntries,
  getSkillResourceActivityItem,
  getToolActivityStatus,
  parseSkillResourceUri
} from '../skillOfficeActivity'

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

  it('groups Office calls only when document kind, normalized file identity and operation mode match', () => {
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

    expect(getOfficeActivityGroupIdentity(firstEdit)?.key).toBe(
      getOfficeActivityGroupIdentity(secondEdit)?.key
    )
    expect(getOfficeActivityGroupIdentity(firstEdit)?.key).not.toBe(
      getOfficeActivityGroupIdentity(view)?.key
    )
    expect(getOfficeActivityGroupIdentity(firstEdit)?.key).not.toBe(
      getOfficeActivityGroupIdentity(otherFile)?.key
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
})
