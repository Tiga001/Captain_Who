import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { FileChangeToolActivity } from '../../features/chat/components/toolActivities/FileChangeToolActivity'
import '../../styles/global.css'
import '../../features/chat/ChatConversationPage.agent.css'

const translations: Record<string, string> = {
  'agent.separator': '，',
  'agent.fileChange.group.processed': '已处理 {count} 个文件',
  'agent.fileChange.group.failedCount': '失败 {count} 个',
  'agent.fileChange.create.row.failed': '新建失败',
  'agent.fileChange.create.row.running': '正在新建',
  'agent.fileChange.create.row.cancelled': '已取消新建',
  'agent.fileChange.failure.generic': '无法安全应用这处修改。',
  'agent.fileChange.failure.conflict': '文件已发生变化，无法安全应用这处修改。',
  'agent.fileChange.failure.fileExists': '文件已存在。',
  'agent.fileChange.failure.staleFile': '文件状态已过期，请重新读取后再试。',
  'agent.fileChange.failure.outcomeUnknown':
    '无法确认修改结果。请先检查文件当前状态，不要直接重试。'
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) => translations[key] ?? key
  })
}))

vi.mock('../../features/storage/storageClient', () => ({
  revealStoredProjectFile: vi.fn()
}))

vi.mock('../../features/agent/agentClient', () => ({
  getAgentFileChangeDiff: vi.fn(),
  readAgentFileChange: vi.fn()
}))

const INTERNAL_ERROR =
  'apply_patch structured_edit_error code=file_exists: /private/workspace/internal.txt'

function applyPatchCall(): AgentToolCall {
  return {
    id: 'patch-create-existing',
    tool: 'apply_patch',
    args: {
      request: {
        action: 'apply',
        operation: 'create',
        filePath: 'existing.txt',
        observationId: 'fobs_missing_target',
        content: 'replacement'
      }
    },
    approvalStatus: 'not_required',
    reason: null
  }
}

describe('FileChange failure presentation', () => {
  it('uses the structured error code without exposing the raw error', async () => {
    const call = applyPatchCall()
    const result: AgentToolResult = {
      callId: call.id,
      tool: call.tool,
      ok: false,
      result: {
        type: 'file_change',
        code: 'file_exists',
        errorCode: 'agent.apply_patch.file_exists',
        category: 'precondition',
        message: '文件已存在。',
        recovery: 'use_update_or_choose_another_path',
        continueWith: null
      },
      error: INTERNAL_ERROR
    }
    const screen = await render(<FileChangeToolActivity call={call} result={result} />)

    screen.container.querySelector<HTMLDetailsElement>('details > summary')?.click()

    await expect.element(screen.getByText('文件已存在。', { exact: true })).toBeVisible()
    expect(screen.container.textContent).not.toContain('structured_edit_error')
    expect(screen.container.textContent).not.toContain('/private/workspace/internal.txt')
  })

  it.each([
    'observation_required',
    'observation_expired',
    'observation_owner_mismatch',
    'observation_path_mismatch',
    'observation_stale'
  ])('maps agent.apply_patch.%s to a safe reread message', async (code) => {
    const call = applyPatchCall()
    const privateCause = `PRIVATE_OBSERVATION_CAUSE:${code}:/private/workspace/README.md`
    const result: AgentToolResult = {
      callId: call.id,
      tool: call.tool,
      ok: false,
      result: {
        type: 'file_change',
        code,
        errorCode: `agent.apply_patch.${code}`,
        category: 'precondition',
        message: privateCause,
        recovery: 'reread_file'
      },
      error: privateCause
    }
    const screen = await render(<FileChangeToolActivity call={call} result={result} />)

    screen.container.querySelector<HTMLDetailsElement>('details > summary')?.click()

    await expect
      .element(screen.getByText('文件状态已过期，请重新读取后再试。', { exact: true }))
      .toBeVisible()
    expect(screen.container.textContent).not.toContain(privateCause)
    expect(screen.container.textContent).not.toContain(code)
  })

  it('fails closed for an unknown code instead of rendering backend text', async () => {
    const call = applyPatchCall()
    const result: AgentToolResult = {
      callId: call.id,
      tool: call.tool,
      ok: false,
      result: { errorCode: 'agent.apply_patch.future_internal_failure' },
      error: 'sensitive provider or filesystem diagnostic'
    }
    const screen = await render(<FileChangeToolActivity call={call} result={result} />)

    screen.container.querySelector<HTMLDetailsElement>('details > summary')?.click()

    await expect.element(screen.getByText('无法安全应用这处修改。', { exact: true })).toBeVisible()
    expect(screen.container.textContent).not.toContain('future_internal_failure')
    expect(screen.container.textContent).not.toContain('sensitive provider')
  })

  it('tells the user to inspect state instead of retrying an outcome-unknown change', async () => {
    const call = applyPatchCall()
    const result: AgentToolResult = {
      callId: call.id,
      tool: call.tool,
      ok: false,
      result: {
        type: 'file_change',
        code: 'outcome_unknown',
        errorCode: 'agent.apply_patch.outcome_unknown',
        category: 'execution',
        message: 'private publication diagnostic',
        recovery: 'do_not_retry'
      },
      error: 'private publication diagnostic'
    }
    const screen = await render(<FileChangeToolActivity call={call} result={result} />)

    screen.container.querySelector<HTMLDetailsElement>('details > summary')?.click()

    await expect
      .element(
        screen.getByText('无法确认修改结果。请先检查文件当前状态，不要直接重试。', {
          exact: true
        })
      )
      .toBeVisible()
    expect(screen.container.textContent).not.toContain('private publication diagnostic')
  })

  it('does not recover a display code from the raw legacy-shaped error text', async () => {
    const call = applyPatchCall()
    const result: AgentToolResult = {
      callId: call.id,
      tool: call.tool,
      ok: false,
      result: { code: 'file_exists' },
      error: INTERNAL_ERROR
    }
    const screen = await render(<FileChangeToolActivity call={call} result={result} />)

    screen.container.querySelector<HTMLDetailsElement>('details > summary')?.click()

    await expect.element(screen.getByText('无法安全应用这处修改。', { exact: true })).toBeVisible()
    expect(screen.container.textContent).not.toContain('文件已存在。')
    expect(screen.container.textContent).not.toContain('structured_edit_error')
  })

  it.each([
    ['cancelled', '已取消新建'],
    ['failed', '新建失败']
  ] as const)(
    'shows a terminal %s Run over an unsettled transaction',
    async (settledStatus, label) => {
      const call = applyPatchCall()
      const screen = await render(
        <FileChangeToolActivity
          call={call}
          settledStatus={settledStatus}
          transaction={{
            schemaVersion: 1,
            transactionId: 'file-change-transaction',
            conversationId: 'conversation',
            projectId: null,
            operation: 'create',
            updateStrategy: null,
            filePath: 'existing.txt',
            status: 'waiting_approval',
            baseRevision: null,
            additions: 1,
            deletions: 0,
            byteCount: 11,
            lineCount: 1,
            mutationCount: 1,
            nextMutationIndex: 1,
            statsFinal: true,
            summary: null,
            createdAt: 1,
            updatedAt: 2
          }}
        />
      )

      expect(screen.container.textContent).toContain(label)
    }
  )
})
