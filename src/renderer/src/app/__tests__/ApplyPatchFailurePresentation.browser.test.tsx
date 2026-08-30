import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import {
  getAgentFileChangeDiff,
  getAgentFileChangeHistoryDiff
} from '../../features/agent/agentClient'
import { FileChangeToolActivity } from '../../features/chat/components/toolActivities/FileChangeToolActivity'
import '../../styles/global.css'
import '../../features/chat/ChatConversationPage.agent.css'

const translations: Record<string, string> = {
  'agent.separator': '，',
  'agent.fileChange.group.processed': '已处理 {count} 个文件',
  'agent.fileChange.group.applied': '已编辑 {count} 个文件',
  'agent.fileChange.group.failedCount': '失败 {count} 个',
  'agent.fileChange.create.row.applied': '已新建',
  'agent.fileChange.create.row.failed': '新建失败',
  'agent.fileChange.create.row.running': '正在新建',
  'agent.fileChange.create.row.cancelled': '已取消新建',
  'agent.fileChange.togglePreview': '展开文件修改',
  'agent.fileChange.loadingPreview': '正在读取修改',
  'agent.fileChange.loadMorePreview': '加载更多',
  'agent.fileChange.historyPreviewUnavailable': '无法读取已保存的修改详情。',
  'chat.copy': '复制',
  'chat.copied': '已复制',
  'files.preview.error': '无法读取修改',
  'gitReview.diff.invalid': '这个文件的差异格式无效，无法安全显示。',
  'gitReview.diff.noHunks': '这个文件没有可显示的文本差异。',
  'gitReview.diff.tooLarge': '这个文件的差异过大，无法安全显示。',
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
  getAgentFileChangeHistoryDiff: vi.fn()
}))

const INTERNAL_ERROR =
  'apply_patch structured_edit_error code=file_exists: /private/workspace/internal.txt'
const CREATED_FILE_PATCH = [
  '--- /dev/null',
  '+++ b/existing.txt',
  '@@ -0,0 +1,1 @@',
  '+replacement'
].join('\n')

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

function appliedFileChangeResult(call: AgentToolCall): AgentToolResult {
  return {
    callId: call.id,
    tool: call.tool,
    ok: true,
    result: {
      schemaVersion: 1,
      status: 'applied',
      outcome: 'applied',
      transactionId: 'file-change-result-only',
      operation: 'create',
      updateStrategy: null,
      filePath: 'existing.txt',
      additions: 1,
      deletions: 0,
      lineCount: 1,
      byteCount: 12,
      revision: 'content-sha256-v1:result-only',
      errorCode: null,
      error: null,
      message: null
    }
  }
}

describe('FileChange failure presentation', () => {
  beforeEach(() => {
    vi.mocked(getAgentFileChangeDiff).mockReset()
    vi.mocked(getAgentFileChangeHistoryDiff).mockReset()
  })

  it('loads a completed result-only Diff and keeps detail text at activity size', async () => {
    const call = applyPatchCall()
    const result = appliedFileChangeResult(call)
    vi.mocked(getAgentFileChangeHistoryDiff).mockResolvedValueOnce({
      conversationId: 'conversation-1',
      assistantMessageId: 'assistant-message-1',
      runId: 'run-1',
      toolCallId: call.id,
      patch: CREATED_FILE_PATCH,
      offset: 0,
      nextOffset: null,
      truncated: false
    })

    const screen = await render(
      <FileChangeToolActivity
        assistantMessageId="assistant-message-1"
        call={call}
        conversationId="conversation-1"
        result={result}
        runId="run-1"
        settledStatus="completed"
      />
    )
    expect(getAgentFileChangeHistoryDiff).not.toHaveBeenCalled()
    expect(getAgentFileChangeDiff).not.toHaveBeenCalled()
    await screen.container.querySelector<HTMLDetailsElement>('details > summary')?.click()

    const activityLabel = screen.container.querySelector<HTMLElement>('.agent-activity__label')
    const itemLine = screen.container.querySelector<HTMLElement>('.file-change-activity__item-line')
    expect(activityLabel).not.toBeNull()
    expect(itemLine).not.toBeNull()
    expect(window.getComputedStyle(itemLine!).fontSize).toBe(
      window.getComputedStyle(activityLabel!).fontSize
    )
    itemLine?.querySelectorAll<HTMLElement>('span').forEach((element) => {
      expect(window.getComputedStyle(element).fontSize).toBe(
        window.getComputedStyle(activityLabel!).fontSize
      )
    })

    const previewToggle = screen.container.querySelector<HTMLButtonElement>(
      '.file-change-activity__toggle'
    )
    expect(previewToggle).not.toBeNull()
    await previewToggle?.click()

    await vi.waitFor(() => {
      expect(getAgentFileChangeHistoryDiff).toHaveBeenCalledWith({
        conversationId: 'conversation-1',
        assistantMessageId: 'assistant-message-1',
        runId: 'run-1',
        toolCallId: call.id,
        offset: 0,
        maxChars: 50_000
      })
    })

    await expect.element(screen.getByText('replacement', { exact: true })).toBeVisible()
    const card = screen.container.querySelector<HTMLElement>('.file-change-diff-card')
    const header = screen.container.querySelector<HTMLElement>('.file-change-diff-card__header')
    const body = screen.container.querySelector<HTMLElement>('.file-change-diff-card__body')
    expect(card).not.toBeNull()
    expect(header?.parentElement).toBe(card)
    expect(body?.parentElement).toBe(card)
    expect(header?.contains(body)).toBe(false)
    expect(body?.style.maxHeight || window.getComputedStyle(body!).maxHeight).toBe('320px')
    expect(
      screen.container.querySelector('.git-review__single-diff')?.getAttribute('data-side')
    ).toBe('new')
    expect(screen.container.querySelectorAll('.git-review__split-pane')).toHaveLength(0)
    expect(screen.container.querySelector('.file-change-diff-card__copy')).not.toBeNull()
    expect(getAgentFileChangeDiff).not.toHaveBeenCalled()
  })

  it.each([
    ['failed', false],
    ['aborted', true]
  ] as const)(
    'does not request history for a generic terminal %s apply_patch result',
    async (settledStatus, cancelled) => {
      const call = applyPatchCall()
      const result: AgentToolResult = {
        callId: call.id,
        tool: call.tool,
        ok: false,
        result: {
          type: 'file_change',
          errorCode: `agent.apply_patch.${settledStatus}`,
          message: '安全错误'
        }
      }
      const screen = await render(
        <FileChangeToolActivity
          assistantMessageId="assistant-message-1"
          call={call}
          cancelled={cancelled}
          conversationId="conversation-1"
          result={result}
          runId="run-1"
          settledStatus={settledStatus === 'failed' ? 'failed' : 'cancelled'}
        />
      )

      await screen.container.querySelector<HTMLDetailsElement>('details > summary')?.click()

      expect(screen.container.querySelector('.file-change-activity__toggle')).toBeNull()
      expect(getAgentFileChangeHistoryDiff).not.toHaveBeenCalled()
    }
  )

  it('does not request terminal history merely because a valid activity mounts again', async () => {
    const call = applyPatchCall()
    const props = {
      assistantMessageId: 'assistant-message-1',
      call,
      conversationId: 'conversation-1',
      result: appliedFileChangeResult(call),
      runId: 'run-1',
      settledStatus: 'completed' as const
    }
    const first = await render(<FileChangeToolActivity {...props} />)

    expect(getAgentFileChangeHistoryDiff).not.toHaveBeenCalled()
    await first.unmount()
    const second = await render(<FileChangeToolActivity {...props} />)

    expect(getAgentFileChangeHistoryDiff).not.toHaveBeenCalled()
    await second.unmount()
  })

  it.each(['unknown result field', 'mismatched Tool Call identity'] as const)(
    'does not offer history for an otherwise complete result with %s',
    async (invalidity) => {
      const call = applyPatchCall()
      const canonical = appliedFileChangeResult(call)
      const result: AgentToolResult =
        invalidity === 'unknown result field'
          ? {
              ...canonical,
              result: {
                ...(canonical.result as Record<string, unknown>),
                internalCause: 'private'
              }
            }
          : { ...canonical, callId: 'different-tool-call' }
      const screen = await render(
        <FileChangeToolActivity
          assistantMessageId="assistant-message-1"
          call={call}
          conversationId="conversation-1"
          result={result}
          runId="run-1"
          settledStatus="completed"
        />
      )

      await screen.container.querySelector<HTMLDetailsElement>('details > summary')?.click()

      expect(screen.container.querySelector('.file-change-activity__toggle')).toBeNull()
      expect(getAgentFileChangeHistoryDiff).not.toHaveBeenCalled()
    }
  )

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
    expect(screen.container.querySelector('.file-change-activity__toggle')).toBeNull()
    expect(getAgentFileChangeDiff).not.toHaveBeenCalled()
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
