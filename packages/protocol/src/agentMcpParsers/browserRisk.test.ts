import { describe, expect, it } from 'vitest'
import {
  parseAgentActionExecutionOutputForHost,
  parseAgentBrowserRiskApproval,
  parseAgentBrowserRiskProposedAction,
  parseAgentEventForHost,
  parsePendingAgentActionSnapshotsForHost
} from '../index'
import { createAgentMcpFixtures } from './fixtures'

const { actionId, callId, browserRiskApproval } = createAgentMcpFixtures()

describe('browser risk Host-boundary contract', () => {
  it('strictly parses the exact approval, event, pending snapshot and execution projection', () => {
    expect(parseAgentBrowserRiskApproval(browserRiskApproval)).toEqual(browserRiskApproval)
    const action = {
      type: 'browser_risk_approval',
      approval: browserRiskApproval
    } as const
    expect(parseAgentBrowserRiskProposedAction(action)).toEqual(action)
    expect(
      parseAgentEventForHost({ type: 'approval_required', runId: 'run-owned', action })
    ).toEqual({ type: 'approval_required', runId: 'run-owned', action })

    const pending = {
      actionId,
      actionType: 'browser_risk_approval',
      toolName: 'browser_navigate',
      toolCallId: callId,
      runId: 'run-owned',
      conversationId: 'conversation-owned',
      assistantMessageId: 'assistant-owned',
      action,
      createdAt: 1_753_843_200_000,
      status: 'pending'
    }
    expect(parsePendingAgentActionSnapshotsForHost([pending])).toEqual([pending])

    const output = parseAgentActionExecutionOutputForHost({
      actionId,
      actionType: 'browser_risk_approval',
      toolName: 'browser_navigate',
      status: 'rejected',
      toolResult: { cookie: 'PRIVATE_COOKIE_CANARY' },
      agentOutput: {
        content: 'The user declined this browser destination.',
        status: 'completed',
        runId: 'run-owned',
        events: [],
        toolDefinitions: [{ description: 'must-not-reach-renderer' }],
        proposedActions: []
      }
    })
    expect(output.toolResult).toBeUndefined()
    expect(output.agentOutput.toolDefinitions).toEqual([])
    expect(JSON.stringify(output)).not.toContain('PRIVATE_COOKIE_CANARY')
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        actionId,
        actionType: 'browser_risk_approval',
        toolName: 'browser_run_code_unsafe',
        status: 'rejected',
        agentOutput: {
          content: '',
          status: 'completed',
          runId: 'run-owned',
          events: [],
          toolDefinitions: [],
          proposedActions: []
        }
      })
    ).toThrow()
  })

  it('rejects hidden authority, inconsistent destinations and malformed frozen identity', () => {
    expect(() =>
      parseAgentBrowserRiskApproval({ ...browserRiskApproval, headers: { authorization: 'x' } })
    ).toThrow(/unexpected field/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        destination: {
          ...browserRiskApproval.destination,
          resolutionFingerprint: `sha256:${'f'.repeat(64)}`
        }
      })
    ).toThrow(/unexpected field/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        destination: {
          ...browserRiskApproval.destination,
          normalizedUrl: 'http://user:secret@127.0.0.1:3000/dashboard'
        }
      })
    ).toThrow(/inconsistent/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        destination: {
          ...browserRiskApproval.destination,
          normalizedUrl: 'http://127.0.0.1:3000/dashboard?token=secret'
        }
      })
    ).toThrow(/inconsistent/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        destination: { ...browserRiskApproval.destination, effectivePort: 3001 }
      })
    ).toThrow(/inconsistent/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        riskKinds: ['loopback', 'loopback']
      })
    ).toThrow(/duplicates/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        expiresAt: browserRiskApproval.createdAt + 899
      })
    ).toThrow(/15 minute/)
    expect(() =>
      parseAgentBrowserRiskApproval({
        ...browserRiskApproval,
        createdAt: 253_402_300_800,
        expiresAt: 253_402_301_700
      })
    ).toThrow(/date range/)
    expect(() =>
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-other',
        action: { type: 'browser_risk_approval', approval: browserRiskApproval }
      })
    ).toThrow(/runId must match/)
    expect(() =>
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-owned',
        action: {
          type: 'browser_risk_approval',
          approval: { ...browserRiskApproval, approvalStatus: 'approved' }
        }
      })
    ).toThrow(/must remain required/)
  })
})
