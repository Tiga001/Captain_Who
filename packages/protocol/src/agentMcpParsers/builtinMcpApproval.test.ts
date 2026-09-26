import { describe, expect, it } from 'vitest'
import {
  parseAgentActionExecutionOutputForHost,
  parseAgentBuiltinMcpToolApproval,
  parseAgentBuiltinMcpToolApprovalProposedAction,
  parseAgentEventForHost,
  parsePendingAgentActionSnapshotsForHost
} from '../index'
import { createAgentMcpFixtures } from './fixtures'

const { actionId, callId, builtinMcpToolApproval } = createAgentMcpFixtures()

describe('built-in MCP Tool approval Host-boundary contract', () => {
  it('strictly parses the safe approval, event, pending snapshot and execution receipt', () => {
    expect(parseAgentBuiltinMcpToolApproval(builtinMcpToolApproval)).toEqual(builtinMcpToolApproval)
    const action = {
      type: 'builtin_mcp_tool_approval',
      approval: builtinMcpToolApproval
    } as const
    expect(parseAgentBuiltinMcpToolApprovalProposedAction(action)).toEqual(action)
    expect(
      parseAgentEventForHost({ type: 'approval_required', runId: 'run-owned', action })
    ).toEqual({ type: 'approval_required', runId: 'run-owned', action })

    const pending = {
      actionId,
      actionType: 'builtin_mcp_tool_approval',
      toolName: 'browser_evaluate',
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
      actionType: 'builtin_mcp_tool_approval',
      toolName: 'browser_evaluate',
      status: 'rejected',
      toolResult: {
        cookie: 'PRIVATE_COOKIE_CANARY',
        storage: 'PRIVATE_STORAGE_CANARY',
        code: 'PRIVATE_SCRIPT_CANARY'
      },
      agentOutput: {
        content: 'The user declined this sensitive browser operation.',
        status: 'completed',
        runId: 'run-owned',
        events: [],
        toolDefinitions: [{ description: 'must-not-reach-renderer' }],
        proposedActions: []
      }
    })
    expect(output.toolResult).toBeUndefined()
    expect(output.agentOutput.toolDefinitions).toEqual([])
    expect(JSON.stringify(output)).not.toContain('PRIVATE_')
  })

  it('rejects hidden authority, identity drift, raw paths and non-canonical risks', () => {
    expect(() =>
      parseAgentBuiltinMcpToolApproval({
        ...builtinMcpToolApproval,
        headers: { authorization: 'PRIVATE_AUTH_CANARY' }
      })
    ).toThrow(/unexpected field/)
    expect(() =>
      parseAgentBuiltinMcpToolApproval({
        ...builtinMcpToolApproval,
        identity: { ...builtinMcpToolApproval.identity, modelName: 'browser_click' }
      })
    ).toThrow(/identities must match/)
    expect(() =>
      parseAgentBuiltinMcpToolApproval({
        ...builtinMcpToolApproval,
        resourceSummary: {
          ...builtinMcpToolApproval.resourceSummary,
          fileBasenames: ['/Users/private/secret.txt']
        }
      })
    ).toThrow(/must remain a basename/)
    expect(() =>
      parseAgentBuiltinMcpToolApproval({
        ...builtinMcpToolApproval,
        riskKinds: ['page_script_execution', 'page_script_execution']
      })
    ).toThrow(/unique and canonical/)
    expect(() =>
      parseAgentBuiltinMcpToolApproval({
        ...builtinMcpToolApproval,
        identity: { ...builtinMcpToolApproval.identity, origin: 'https://fixture.example/path' }
      })
    ).toThrow(/origin/)
  })

  it('fails closed for mismatched or non-pending hydration identities', () => {
    const action = {
      type: 'builtin_mcp_tool_approval',
      approval: builtinMcpToolApproval
    } as const
    const pending = {
      actionId,
      actionType: 'builtin_mcp_tool_approval',
      toolName: 'browser_evaluate',
      toolCallId: callId,
      runId: 'run-owned',
      conversationId: null,
      assistantMessageId: null,
      action,
      createdAt: 1_753_843_200_000,
      status: 'pending'
    }
    expect(() =>
      parsePendingAgentActionSnapshotsForHost([{ ...pending, status: 'approved' }])
    ).toThrow(/expected pending/)
    expect(() =>
      parsePendingAgentActionSnapshotsForHost([{ ...pending, toolName: 'browser_cookie_get' }])
    ).toThrow(/expected browser_evaluate/)
    expect(() =>
      parsePendingAgentActionSnapshotsForHost([{ ...pending, toolCallId: null }])
    ).toThrow(/expected a string/)
    expect(() =>
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-other',
        action
      })
    ).toThrow(/runId must match/)
  })
})
