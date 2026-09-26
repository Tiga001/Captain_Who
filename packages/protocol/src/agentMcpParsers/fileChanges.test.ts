import { describe, expect, it } from 'vitest'
import {
  parseAgentActionExecutionOutputForHost,
  parseAgentEventForHost,
  parseAgentFileChangeContentPageForHost,
  parseAgentFileChangeDiffPageForHost,
  parseAgentFileChangeHistoryDiffPageForHost
} from '../index'

describe('Renderer-safe Direct file-change proposal contract', () => {
  const fileChange = {
    schemaVersion: 1,
    id: 'file-change-direct-current',
    transactionId: 'file-change-transaction-current',
    operation: 'update',
    updateStrategy: null,
    filePath: 'README.md',
    inlineDiff: { patch: '@@ -1 +1 @@\n-old\n+new', truncated: false },
    baseRevision: 'content-sha256-v1:current',
    summary: 'Update the heading',
    additions: 1,
    deletions: 1,
    lineCount: 1,
    byteCount: 4,
    approvalStatus: 'required'
  } as const

  it('round-trips only the public FileChange projection', () => {
    const event = {
      type: 'approval_required',
      runId: 'run-owned',
      action: { type: 'file_change', fileChange }
    } as const

    expect(parseAgentEventForHost(event)).toEqual(event)
    expect(JSON.stringify(parseAgentEventForHost(event))).not.toContain('execution')
  })

  it('rejects Host-private execution authority instead of adding it to the public DTO', () => {
    expect(() =>
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-owned',
        action: {
          type: 'file_change',
          fileChange: {
            ...fileChange,
            execution: {
              schemaVersion: 1,
              canonicalTarget: '/private/workspace/README.md',
              baseContent: 'PRIVATE_BASE_CONTENT'
            }
          }
        }
      })
    ).toThrow(/unexpected field execution/)
  })

  it('rejects missing, extra, unknown-version and old Direct shapes', () => {
    const current = {
      type: 'approval_required',
      runId: 'run-owned',
      action: { type: 'file_change', fileChange }
    }
    expect(() =>
      parseAgentEventForHost({
        ...current,
        action: { type: 'file_change', fileChange: { ...fileChange, schemaVersion: 2 } }
      })
    ).toThrow(/schemaVersion/)
    const missingTransaction: Partial<typeof fileChange> = { ...fileChange }
    delete missingTransaction.transactionId
    expect(() =>
      parseAgentEventForHost({
        ...current,
        action: { type: 'file_change', fileChange: missingTransaction }
      })
    ).toThrow(/transactionId/)
    expect(() =>
      parseAgentEventForHost({
        ...current,
        action: { type: 'file_change', fileChange: { ...fileChange, patch: 'old-shape' } }
      })
    ).toThrow(/unexpected field patch/)
    expect(() =>
      parseAgentEventForHost({
        ...current,
        action: {
          type: 'file_change',
          fileChange: { ...fileChange, approvalStatus: 'not_required' }
        }
      })
    ).toThrow(/approvalStatus/)
    expect(() =>
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-owned',
        action: { type: 'diff', diff: fileChange }
      })
    ).toThrow(/unknown proposed action type diff/)
  })
})

describe('Renderer-safe Staged file-change proposal contract', () => {
  const fileChange = {
    schemaVersion: 1,
    id: 'call-staged-commit',
    transactionId: 'file-change-staged-v1:current',
    operation: 'update',
    updateStrategy: 'rewrite',
    filePath: 'reports/large.md',
    inlineDiff: null,
    baseRevision: 'content-sha256-v1:current',
    summary: 'Publish the assembled report',
    additions: 12_000,
    deletions: 8_000,
    lineCount: 12_000,
    byteCount: 4 * 1024 * 1024,
    approvalStatus: 'required'
  } as const

  it('round-trips only metadata needed for approval and paged Diff reads', () => {
    const event = {
      type: 'approval_required',
      runId: 'run-owned',
      action: { type: 'file_change', fileChange }
    } as const

    expect(parseAgentEventForHost(event)).toEqual(event)
    expect(JSON.stringify(parseAgentEventForHost(event))).not.toContain('execution')
  })

  it('rejects private execution authority and inline large bodies', () => {
    for (const extra of [
      {
        execution: {
          canonicalTarget: '/private/workspace/reports/large.md',
          targetContent: 'PRIVATE_STAGED_TARGET_CANARY'
        }
      },
      { patch: 'PRIVATE_STAGED_DIFF_CANARY' }
    ]) {
      expect(() =>
        parseAgentEventForHost({
          type: 'approval_required',
          runId: 'run-owned',
          action: {
            type: 'file_change',
            fileChange: { ...fileChange, ...extra }
          }
        })
      ).toThrow(/unexpected field/)
    }
  })

  it('requires the staged update strategy and rejects the old FileWrite action', () => {
    expect(() =>
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-owned',
        action: {
          type: 'file_change',
          fileChange: { ...fileChange, updateStrategy: null }
        }
      })
    ).toThrow(/staged update requires updateStrategy/)
    expect(() =>
      parseAgentEventForHost({
        type: 'approval_required',
        runId: 'run-owned',
        action: { type: 'file_write', fileWrite: fileChange }
      })
    ).toThrow(/unknown proposed action type file_write/)
  })
})

describe('Renderer-safe FileChange execution result contract', () => {
  const result = {
    schemaVersion: 1,
    status: 'applied',
    outcome: 'applied',
    transactionId: 'file-change-result-current',
    operation: 'update',
    updateStrategy: null,
    filePath: 'README.md',
    additions: 1,
    deletions: 1,
    lineCount: 1,
    byteCount: 4,
    revision: 'content-sha256-v1:target',
    errorCode: null,
    error: null,
    message: null
  } as const
  const output = {
    actionId: 'file-change-action-current',
    actionType: 'file_change',
    toolName: 'apply_patch',
    status: 'applied',
    fileChangeResult: result,
    agentOutput: {
      content: '',
      status: 'running',
      runId: 'run-owned',
      events: [],
      toolDefinitions: [],
      proposedActions: []
    }
  } as const

  it('round-trips the only current result shape', () => {
    expect(parseAgentActionExecutionOutputForHost(output)).toEqual(output)
  })

  it('rejects missing, extra, unknown-version and illegal terminal combinations', () => {
    const missingRevision: Partial<typeof result> = { ...result }
    delete missingRevision.revision
    expect(() =>
      parseAgentActionExecutionOutputForHost({ ...output, fileChangeResult: missingRevision })
    ).toThrow(/revision/)
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        ...output,
        fileChangeResult: { ...result, internalCause: 'private' }
      })
    ).toThrow(/unexpected field internalCause/)
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        ...output,
        fileChangeResult: { ...result, schemaVersion: 2 }
      })
    ).toThrow(/schemaVersion/)
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        ...output,
        fileChangeResult: {
          ...result,
          outcome: 'outcome_unknown',
          revision: null,
          errorCode: 'outcome_unknown'
        }
      })
    ).toThrow(/illegal terminal state/)
    expect(() => parseAgentActionExecutionOutputForHost({ ...output, status: 'failed' })).toThrow(
      /status does not match/
    )
  })

  it('accepts current cancellation and commit-unknown status/result pairs', () => {
    expect(
      parseAgentActionExecutionOutputForHost({
        ...output,
        status: 'cancelled',
        fileChangeResult: {
          ...result,
          status: 'aborted',
          outcome: 'definitely_not_executed',
          revision: null,
          errorCode: 'aborted',
          error: 'The file change was cancelled.'
        }
      }).status
    ).toBe('cancelled')
    expect(
      parseAgentActionExecutionOutputForHost({
        ...output,
        status: 'outcome_unknown',
        fileChangeResult: {
          ...result,
          status: 'outcome_unknown',
          outcome: 'outcome_unknown',
          revision: null,
          errorCode: 'outcome_unknown',
          error: 'The file change outcome could not be proven.'
        }
      }).status
    ).toBe('outcome_unknown')
  })

  it('rejects cross-action and old result fields instead of silently dropping them', () => {
    expect(() =>
      parseAgentActionExecutionOutputForHost({ ...output, toolResult: { ok: true } })
    ).toThrow(/toolResult are forbidden/)
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        ...output,
        patchResult: result
      })
    ).toThrow(/unexpected field patchResult/)
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        ...output,
        fileWriteResult: result
      })
    ).toThrow(/unexpected field fileWriteResult/)
    expect(() =>
      parseAgentActionExecutionOutputForHost({
        ...output,
        actionType: 'mcp_tool_call',
        toolName: 'mcp__fixture__tool'
      })
    ).toThrow(/fileChangeResult is only valid/)
  })
})

describe('Renderer-safe FileChange snapshot terminal states', () => {
  const snapshot = {
    schemaVersion: 1,
    transactionId: 'file-change-snapshot-current',
    conversationId: 'conversation-owned',
    projectId: null,
    filePath: 'README.md',
    operation: 'update',
    updateStrategy: 'modify',
    baseRevision: 'content-sha256-v1:base',
    additions: 1,
    deletions: 1,
    lineCount: 1,
    byteCount: 4,
    mutationCount: 1,
    nextMutationIndex: 1,
    statsFinal: true,
    summary: null,
    createdAt: 10,
    updatedAt: 11
  } as const

  it.each(['already_applied', 'outcome_unknown'] as const)(
    'accepts the current %s transaction terminal status',
    (status) => {
      const event = {
        type: 'file_change_updated',
        runId: 'run-owned',
        fileChange: { ...snapshot, status }
      } as const
      expect(parseAgentEventForHost(event)).toEqual(event)
    }
  )

  it('rejects missing, extra, unknown-version and illegal snapshot combinations', () => {
    const event = (fileChange: unknown) => ({
      type: 'file_change_updated',
      runId: 'run-owned',
      fileChange
    })
    const current = { ...snapshot, status: 'applied' }
    const missingNullable: Partial<typeof current> = { ...current }
    delete missingNullable.projectId
    expect(() => parseAgentEventForHost(event(missingNullable))).toThrow(/projectId/)
    expect(() => parseAgentEventForHost(event({ ...current, internalCause: 'private' }))).toThrow(
      /unexpected field internalCause/
    )
    expect(() => parseAgentEventForHost(event({ ...current, schemaVersion: 2 }))).toThrow(
      /schemaVersion/
    )
    expect(() => parseAgentEventForHost(event({ ...current, updateStrategy: null }))).toThrow(
      /operation\/updateStrategy/
    )
    expect(() =>
      parseAgentEventForHost(event({ ...current, status: 'drafting', statsFinal: true }))
    ).toThrow(/status\/statsFinal/)
  })
})

describe('Renderer-safe FileChange paged RPC contract', () => {
  const snapshot = {
    schemaVersion: 1,
    transactionId: 'file-change-page-current',
    conversationId: 'conversation-owned',
    projectId: null,
    filePath: 'README.md',
    operation: 'update',
    updateStrategy: 'rewrite',
    status: 'waiting_approval',
    baseRevision: 'content-sha256-v1:base',
    additions: 1,
    deletions: 1,
    lineCount: 1,
    byteCount: 4,
    mutationCount: 1,
    nextMutationIndex: 1,
    statsFinal: true,
    summary: null,
    createdAt: 10,
    updatedAt: 11
  } as const

  it('round-trips required-nullable pagination fields', () => {
    const contentPage = {
      fileChange: snapshot,
      content: 'new\n',
      offset: 0,
      nextOffset: null,
      truncated: false
    }
    const diffPage = {
      transactionId: snapshot.transactionId,
      patch: '+new\n',
      offset: 0,
      nextOffset: 5,
      truncated: true
    }
    expect(parseAgentFileChangeContentPageForHost(contentPage)).toEqual(contentPage)
    expect(parseAgentFileChangeDiffPageForHost(diffPage)).toEqual(diffPage)
    const historyPage = {
      conversationId: 'conversation-owned',
      assistantMessageId: 'assistant-message-owned',
      runId: 'run-owned',
      toolCallId: 'tool-call-owned',
      patch: '+new\n',
      offset: 0,
      nextOffset: null,
      truncated: false
    }
    expect(parseAgentFileChangeHistoryDiffPageForHost(historyPage)).toEqual(historyPage)
  })

  it('rejects missing, extra and incomplete page chains', () => {
    expect(() =>
      parseAgentFileChangeDiffPageForHost({
        transactionId: snapshot.transactionId,
        patch: '+new\n',
        offset: 0,
        truncated: false
      })
    ).toThrow(/nextOffset/)
    expect(() =>
      parseAgentFileChangeDiffPageForHost({
        transactionId: snapshot.transactionId,
        patch: '+new\n',
        offset: 0,
        nextOffset: null,
        truncated: true
      })
    ).toThrow(/invalid page chain/)
    expect(() =>
      parseAgentFileChangeContentPageForHost({
        fileChange: snapshot,
        content: 'new\n',
        offset: 0,
        nextOffset: null,
        truncated: false,
        internalCause: 'private'
      })
    ).toThrow(/unexpected field internalCause/)
    expect(() =>
      parseAgentFileChangeHistoryDiffPageForHost({
        conversationId: 'conversation-owned',
        assistantMessageId: 'assistant-message-owned',
        runId: 'run-owned',
        toolCallId: 'tool-call-owned',
        patch: '+new\n',
        offset: 0,
        nextOffset: null,
        truncated: false,
        actionJson: { executionCredential: 'private' }
      })
    ).toThrow(/unexpected field actionJson/)
  })
})
