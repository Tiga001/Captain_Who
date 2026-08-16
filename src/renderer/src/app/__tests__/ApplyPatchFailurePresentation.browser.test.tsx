import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ApplyPatchToolActivity } from '../../features/chat/components/toolActivities/ApplyPatchToolActivity'
import '../../styles/global.css'
import '../../features/chat/ChatConversationPage.agent.css'

const translations: Record<string, string> = {
  'agent.separator': '，',
  'agent.patch.group.processed': '已处理 {count} 个文件',
  'agent.patch.group.failedCount': '失败 {count} 个',
  'agent.patch.create.row.failed': '新建失败',
  'agent.patch.failure.generic': '无法安全应用这处修改。',
  'agent.patch.failure.conflict': '文件已发生变化，无法安全应用这处修改。',
  'agent.patch.failure.fileExists': '文件已存在。'
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) => translations[key] ?? key
  })
}))

vi.mock('../../features/storage/storageClient', () => ({
  revealStoredProjectFile: vi.fn()
}))

const INTERNAL_ERROR =
  'apply_patch structured_edit_error code=file_exists: /private/workspace/internal.txt'

function applyPatchCall(): AgentToolCall {
  return {
    id: 'patch-create-existing',
    tool: 'apply_patch',
    args: {
      operation: 'create',
      filePath: 'existing.txt',
      content: 'replacement'
    },
    approvalStatus: 'not_required',
    reason: null
  }
}

describe('apply_patch failure presentation', () => {
  it('uses the structured error code without exposing the raw error', async () => {
    const call = applyPatchCall()
    const result: AgentToolResult = {
      callId: call.id,
      tool: call.tool,
      ok: false,
      result: {
        type: 'structured_edit_error',
        code: 'file_exists',
        errorCode: 'agent.apply_patch.file_exists',
        recovery: 'useUpdateOrChooseAnotherPath'
      },
      error: INTERNAL_ERROR
    }
    const screen = await render(<ApplyPatchToolActivity call={call} result={result} />)

    screen.container.querySelector<HTMLDetailsElement>('details > summary')?.click()

    await expect.element(screen.getByText('文件已存在。', { exact: true })).toBeVisible()
    expect(screen.container.textContent).not.toContain('structured_edit_error')
    expect(screen.container.textContent).not.toContain('/private/workspace/internal.txt')
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
    const screen = await render(<ApplyPatchToolActivity call={call} result={result} />)

    screen.container.querySelector<HTMLDetailsElement>('details > summary')?.click()

    await expect.element(screen.getByText('无法安全应用这处修改。', { exact: true })).toBeVisible()
    expect(screen.container.textContent).not.toContain('future_internal_failure')
    expect(screen.container.textContent).not.toContain('sensitive provider')
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
    const screen = await render(<ApplyPatchToolActivity call={call} result={result} />)

    screen.container.querySelector<HTMLDetailsElement>('details > summary')?.click()

    await expect.element(screen.getByText('无法安全应用这处修改。', { exact: true })).toBeVisible()
    expect(screen.container.textContent).not.toContain('文件已存在。')
    expect(screen.container.textContent).not.toContain('structured_edit_error')
  })
})
