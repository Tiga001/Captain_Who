import { describe, expect, it } from 'vitest'
import { parseAgentApproveActionRequest } from './agentActionParsers'

describe('Agent approve-action request parser', () => {
  it.each(['singleAction', 'remainingApplyPatchInRun'] as const)(
    'accepts the current %s approval scope',
    (approvalScope) => {
      const request = {
        runId: 'run-current',
        actionId: 'action-current',
        approvalScope
      }

      expect(parseAgentApproveActionRequest(request)).toEqual(request)
    }
  )

  it.each([
    { runId: 'run-current', actionId: 'action-current' },
    { runId: 'run-current', actionId: 'action-current', approvalScope: null },
    { runId: '', actionId: 'action-current', approvalScope: 'singleAction' },
    { runId: '   ', actionId: 'action-current', approvalScope: 'singleAction' },
    { runId: 'run-current', actionId: '', approvalScope: 'singleAction' },
    { runId: 'run-current', actionId: '\t', approvalScope: 'singleAction' },
    { runId: ' run-current', actionId: 'action-current', approvalScope: 'singleAction' },
    { runId: 'run-current', actionId: 'action-current ', approvalScope: 'singleAction' },
    { runId: 'run-current', actionId: 'action-current', approvalScope: 'remaining_file_changes' },
    {
      runId: 'run-current',
      actionId: 'action-current',
      approval_scope: 'singleAction'
    },
    {
      runId: 'run-current',
      actionId: 'action-current',
      approvalScope: 'singleAction',
      rememberForRun: true
    }
  ])('rejects a missing, unknown, snake_case, or extra field: %#', (request) => {
    expect(() => parseAgentApproveActionRequest(request)).toThrow(/Invalid Agent approve action/)
  })
})
