import { describe, expect, it, vi } from 'vitest'
import type { AgentFileChangeSnapshot, AgentToolCall } from '@mycopilot/protocol'
import { ensureAgentRun } from '../../agentRun/agentEventReducer'
import type { BasicToolCategory, BasicToolItem } from '../components/basicToolTimeline'
import {
  deriveBasicToolItemStatus,
  getBasicToolGroupPresentation
} from '../components/toolActivities/basicToolStatus'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

function call(id: string, tool = 'read_file'): AgentToolCall {
  return { id, tool, args: { path: `${id}.txt` }, approvalStatus: 'not_required', reason: null }
}
function item(id: string, ordinal: number, category: BasicToolCategory = 'read'): BasicToolItem {
  return { id, callIds: [id], firstOrdinal: ordinal, latestOrdinal: ordinal, category }
}
const transaction: AgentFileChangeSnapshot = {
  schemaVersion: 1,
  transactionId: 'transaction',
  conversationId: 'conversation',
  projectId: null,
  filePath: 'example.txt',
  operation: 'update',
  updateStrategy: 'modify',
  status: 'drafting',
  baseRevision: null,
  additions: 3,
  deletions: 1,
  lineCount: 5,
  byteCount: 20,
  mutationCount: 1,
  nextMutationIndex: 1,
  statsFinal: false,
  summary: null,
  createdAt: 1,
  updatedAt: 2
}

describe('mixed basic tool status selection', () => {
  it('selects newest active work and falls back to an older running action after newer work settles', () => {
    const run = ensureAgentRun(undefined, 'run', 'running')
    run.toolCalls = [call('older'), call('newer', 'search_code')]
    const items = [item('older', 0), item('newer', 1, 'search')]
    expect(getBasicToolGroupPresentation(run, items).selected?.call.id).toBe('newer')
    run.toolResults = [{ callId: 'newer', tool: 'search_code', ok: true }]
    expect(getBasicToolGroupPresentation(run, items).selected?.call.id).toBe('older')
    run.toolResults[0].ok = false
    const failed = getBasicToolGroupPresentation(run, items)
    expect(failed.selected?.call.id).toBe('older')
    expect(failed.attention).toBe('failed')
    expect(failed.items.map((status) => status.call.id)).toEqual(['older', 'newer'])
  })

  it('prioritizes approval over later running work, then resumes latest active selection', () => {
    const run = ensureAgentRun(undefined, 'run', 'running')
    run.toolCalls = [
      { ...call('approval', 'run_command'), approvalStatus: 'required' },
      call('later')
    ]
    const items = [item('approval', 0, 'command'), item('later', 1)]
    expect(getBasicToolGroupPresentation(run, items).selected?.phase).toBe('awaiting_approval')
    run.toolCalls[0].approvalStatus = 'approved'
    expect(getBasicToolGroupPresentation(run, items).selected?.call.id).toBe('later')
  })

  it('keeps drafting, ready, and applying stages distinct and does not call uncommitted edits applied', () => {
    const run = ensureAgentRun(undefined, 'run', 'running')
    run.toolCalls = [
      { ...call('edit', 'apply_patch'), args: { request: { transactionId: 'transaction' } } }
    ]
    run.fileChanges = [{ ...transaction }]
    const edit = item('edit', 0, 'edit')
    expect(deriveBasicToolItemStatus(run, edit)).toMatchObject({
      phase: 'preparing',
      isActive: true,
      editOperation: 'update',
      editCounts: { additions: 3, deletions: 1 }
    })
    run.fileChanges[0].status = 'ready'
    expect(deriveBasicToolItemStatus(run, edit)).toMatchObject({
      phase: 'ready',
      isActive: false,
      isPending: true,
      editOperation: 'update'
    })
    run.toolCalls.push(call('read'))
    expect(getBasicToolGroupPresentation(run, [edit, item('read', 1)]).selected?.call.id).toBe(
      'read'
    )
    run.fileChanges[0].status = 'waiting_approval'
    expect(getBasicToolGroupPresentation(run, [edit, item('read', 1)]).selected).toMatchObject({
      call: { id: 'edit' },
      phase: 'awaiting_approval',
      isActive: false,
      isPending: true,
      editOperation: 'update'
    })
    run.fileChanges[0].status = 'applying'
    expect(deriveBasicToolItemStatus(run, edit)?.phase).toBe('running')
    run.status = 'completed'
    expect(deriveBasicToolItemStatus(run, edit)?.phase).toBe('cancelled')
  })

  it('counts completed creates, updates, and deletes separately, with staged calls counted once', () => {
    const run = ensureAgentRun(undefined, 'run', 'completed')
    run.toolCalls = [
      {
        ...call('begin-create', 'apply_patch'),
        args: { request: { action: 'begin', transactionId: 'create', operation: 'create' } }
      },
      {
        ...call('append-create', 'apply_patch'),
        args: { request: { action: 'append', transactionId: 'create', content: 'new content' } }
      },
      {
        ...call('commit-create', 'apply_patch'),
        args: { request: { action: 'commit', transactionId: 'create' } }
      },
      {
        ...call('update', 'apply_patch'),
        args: { request: { action: 'apply', transactionId: 'update' } }
      },
      {
        ...call('delete', 'apply_patch'),
        args: { request: { action: 'apply', transactionId: 'delete' } }
      }
    ]
    run.fileChanges = (['create', 'update', 'delete'] as const).map((operation) => ({
      ...transaction,
      transactionId: operation,
      filePath: `${operation}.txt`,
      operation,
      status: 'applied'
    }))
    const presentation = getBasicToolGroupPresentation(run, [
      {
        ...item('begin-create', 0, 'edit'),
        callIds: ['begin-create', 'append-create', 'commit-create'],
        latestOrdinal: 2
      },
      item('update', 3, 'edit'),
      item('delete', 4, 'edit')
    ])

    expect(presentation.selected).toBeUndefined()
    expect(presentation.outcomes).toEqual([])
    expect(presentation.categoryCounts).toEqual({ create: 1, edit: 1, delete: 1 })
    expect(
      presentation.items.map(({ call, phase, editOperation }) => [call.id, phase, editOperation])
    ).toEqual([
      ['commit-create', 'completed', 'create'],
      ['update', 'completed', 'update'],
      ['delete', 'completed', 'delete']
    ])
  })

  it('honors a command running receipt after parent cancellation and keeps unknown outcome distinct', () => {
    const run = ensureAgentRun(undefined, 'run', 'cancelled')
    run.toolCalls = [call('command', 'run_command')]
    run.toolResults = [
      {
        callId: 'command',
        tool: 'run_command',
        ok: true,
        result: { status: 'running', sessionId: 'session' }
      }
    ]
    const command = item('command', 0, 'command')
    expect(getBasicToolGroupPresentation(run, [command]).selected?.phase).toBe('running')
    run.commandSessions = {
      command: {
        callId: 'command',
        sessionId: 'session',
        status: 'outcome_unknown',
        latestSequence: 1,
        outputTruncated: false
      }
    }
    const unknown = getBasicToolGroupPresentation(run, [command])
    expect(unknown.selected).toBeUndefined()
    expect(unknown.items[0].phase).toBe('outcome_unknown')
    expect(unknown.outcomes).toEqual(['unknown'])
  })

  it.each([true, false])(
    'preserves a command outcome_unknown result without a session (ok=%s)',
    (ok) => {
      const run = ensureAgentRun(undefined, 'run', 'completed')
      run.toolCalls = [call('command', 'run_command')]
      run.toolResults = [
        {
          callId: 'command',
          tool: 'run_command',
          ok,
          result: { status: 'outcome_unknown', command: 'check project' }
        }
      ]
      const presentation = getBasicToolGroupPresentation(run, [item('command', 0, 'command')])
      expect(presentation.selected).toBeUndefined()
      expect(presentation.items[0].phase).toBe('outcome_unknown')
      expect(presentation.outcomes).toEqual(['unknown'])
    }
  )

  it('summarizes settled categories and abnormal outcomes without success/failure counters', () => {
    const run = ensureAgentRun(undefined, 'run', 'completed')
    run.toolCalls = [call('read'), call('search', 'search_files')]
    run.toolResults = [
      { callId: 'read', tool: 'read_file', ok: true },
      { callId: 'search', tool: 'search_files', ok: false }
    ]
    const presentation = getBasicToolGroupPresentation(run, [
      item('read', 0),
      item('search', 1, 'search')
    ])
    expect(presentation.selected).toBeUndefined()
    expect(presentation.categoryCounts).toEqual({ read: 1, search: 1 })
    expect(presentation.outcomes).toEqual(['failed'])
    expect(presentation.attention).toBeUndefined()
  })
})
