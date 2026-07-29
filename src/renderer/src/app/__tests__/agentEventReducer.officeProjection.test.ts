import type { AgentProposedAction } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import type { ChatMessage } from '../../features/chat/chatTypes'
import { applyAgentEventToChatMessage } from '../../features/agentRun/agentEventReducer'

function assistantMessage(): ChatMessage {
  return {
    id: 'assistant-message',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending'
  }
}

function projectApproval(action: AgentProposedAction) {
  const message = applyAgentEventToChatMessage(assistantMessage(), {
    type: 'approval_required',
    runId: 'office-run',
    action
  })
  return message.agentRun?.toolCalls[0]?.args
}

describe('Office frozen action projection', () => {
  it('reconstructs the nested typed v4 model call without exposing execution authority', () => {
    const action: AgentProposedAction = {
      type: 'office_operation',
      officeOperation: {
        schemaVersion: 4,
        id: 'office-add',
        approvalStatus: 'required',
        reason: 'Add the quarterly summary chart',
        prepared: {
          schemaVersion: 4,
          providerId: 'officecli',
          engineRevision: 'sha256:engine',
          access: 'fileWrite',
          request: {
            documentKind: 'spreadsheet',
            operation: 'add',
            documentPath: 'budget.xlsx',
            parameters: {
              type: 'add',
              parent: '/Sheet1',
              elementType: 'chart',
              copyFrom: '/Sheet1/A1:D4',
              position: { type: 'after', target: '/Sheet1/table[1]' },
              properties: {
                title: 'Quarterly summary',
                image: { resourcePath: 'assets/logo.png' }
              },
              force: true
            },
            destinationPath: '@documents/budget-with-chart.xlsx',
            timeoutMs: 45_000
          },
          argv: [
            'add',
            '/private/staging/budget.xlsx',
            '/Sheet1',
            'chart',
            '--prop',
            'title=Quarterly summary'
          ],
          paths: []
        }
      }
    }

    expect(projectApproval(action)).toEqual({
      request: {
        operation: 'add',
        filePath: 'budget.xlsx',
        parent: '/Sheet1',
        element: 'chart',
        copyFrom: '/Sheet1/A1:D4',
        placement: { type: 'after', target: '/Sheet1/table[1]' },
        properties: {
          title: 'Quarterly summary',
          image: { resourcePath: 'assets/logo.png' }
        },
        overrideProtection: true,
        destinationPath: '@documents/budget-with-chart.xlsx',
        timeoutMs: 45_000
      },
      reason: 'Add the quarterly summary chart'
    })
  })

  it('marks historical v3 requests without projecting legacy OfficeCLI arguments', () => {
    const action: AgentProposedAction = {
      type: 'office_operation',
      officeOperation: {
        schemaVersion: 3,
        id: 'office-legacy',
        approvalStatus: 'required',
        reason: 'Render the historical workbook',
        prepared: {
          schemaVersion: 3,
          providerId: 'officecli',
          engineRevision: 'legacy-engine',
          access: 'fileWrite',
          request: {
            documentKind: 'spreadsheet',
            operation: 'view',
            documentPath: 'budget.xlsx',
            arguments: ['screenshot', '--pages', '1-5'],
            outputPath: 'preview.png'
          },
          argv: ['view', 'budget.xlsx', 'screenshot', '--pages', '1-5', '-o', 'preview.png'],
          paths: []
        }
      }
    }

    expect(projectApproval(action)).toEqual({
      request: {
        operation: 'view',
        filePath: 'budget.xlsx',
        legacyRequest: true,
        outputPath: 'preview.png'
      },
      reason: 'Render the historical workbook'
    })
  })

  it('projects screenshot layout as the typed grid object instead of a provider flag', () => {
    const action: AgentProposedAction = {
      type: 'office_operation',
      officeOperation: {
        schemaVersion: 4,
        id: 'office-render',
        approvalStatus: 'required',
        reason: 'Render a five-slide contact sheet',
        prepared: {
          schemaVersion: 4,
          providerId: 'officecli',
          engineRevision: 'sha256:engine',
          access: 'fileWrite',
          request: {
            documentKind: 'presentation',
            operation: 'view',
            documentPath: 'deck.pptx',
            parameters: {
              type: 'view',
              mode: 'screenshot',
              pages: [{ start: 1, end: 5 }],
              grid: { mode: 'columns', columns: 3 }
            },
            outputPath: 'preview.png'
          },
          argv: ['view', 'deck.pptx', 'screenshot', '--grid', '3', '-o', 'preview.png'],
          paths: []
        }
      }
    }

    expect(projectApproval(action)).toEqual({
      request: {
        operation: 'view',
        filePath: 'deck.pptx',
        mode: 'screenshot',
        pages: [{ start: 1, end: 5 }],
        grid: { mode: 'columns', columns: 3 },
        outputPath: 'preview.png'
      },
      reason: 'Render a five-slide contact sheet'
    })
  })
})
