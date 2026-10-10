import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { IpcMainInvokeEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { WORKFLOW_TEMPLATE_MARKDOWN_MAX_BYTES } from '@mycopilot/protocol'
import {
  registerWorkflowTemplateIpc,
  type WorkflowTemplateIpcDependencies
} from '../ipc/workflowTemplateIpc'
import type { TrustedIpcMain } from '../ipc/trustedIpc'

const imported = { records: [], issues: [], importedTemplateId: 'new-template' }
const markdown = `# 组织：研究组织

## 简介
研究问题并形成报告。

## 公共背景
结论需要有来源支撑。

## 部门：研究部

### 成员：研究员
- 职级：30
- 管理身份：普通成员

#### 接收内容
问题和参考资料。

#### 职责
核验事实并整理证据。

#### 交付要求
提交研究报告和来源。
`
const exported = {
  records: [],
  issues: [],
  exportedTemplate: { markdown, suggestedFileName: 'team.md' }
}
const exportInput = { id: 'template', expectedRevision: 3 }
const event = {} as IpcMainInvokeEvent

function harness() {
  const handlers = new Map<string, Parameters<TrustedIpcMain['handle']>[1]>()
  const core = { requestWorkflows: vi.fn().mockResolvedValue(imported) }
  const session = { assertCurrent: vi.fn(), dispose: vi.fn() }
  const dependencies: WorkflowTemplateIpcDependencies = {
    selectImportPath: vi.fn().mockResolvedValue(null),
    selectExportPath: vi.fn().mockResolvedValue(null),
    beginSession: vi.fn(() => session)
  }
  registerWorkflowTemplateIpc(
    { handle: (channel, handler) => handlers.set(channel, handler), on: vi.fn() },
    core,
    dependencies
  )
  return {
    core,
    session,
    dependencies,
    importTemplate: (...args: unknown[]) =>
      handlers.get(HOST_CHANNELS.agent.workflowTemplateImport)!(event, ...args),
    exportTemplate: (input: unknown = exportInput) =>
      handlers.get(HOST_CHANNELS.agent.workflowTemplateExport)!(event, input)
  }
}

function failure(code: string) {
  return { ok: false, error: { message: code, data: { code } } }
}

describe('native organization template Markdown IPC', () => {
  let directory: string
  beforeEach(async () => {
    directory = await mkdtemp(join(tmpdir(), 'organization-template-ipc-'))
  })
  afterEach(async () => {
    await rm(directory, { recursive: true, force: true })
  })

  it('silently cancels imports without calling Core', async () => {
    const h = harness()
    await expect(h.importTemplate()).resolves.toEqual({ ok: true, value: null })
    expect(h.core.requestWorkflows).not.toHaveBeenCalled()
    expect(h.session.dispose).toHaveBeenCalledOnce()
  })

  it('reads selected UTF-8 bytes and returns Core’s authoritative imported identity', async () => {
    const h = harness()
    const path = join(directory, 'selected.md')
    await writeFile(path, markdown)
    vi.mocked(h.dependencies.selectImportPath).mockResolvedValue(path)
    await expect(h.importTemplate()).resolves.toEqual({ ok: true, value: imported })
    expect(h.dependencies.selectImportPath).toHaveBeenCalledExactlyOnceWith(event)
    expect(h.core.requestWorkflows).toHaveBeenCalledExactlyOnceWith({
      operation: 'importTemplateMarkdown',
      markdown
    })
  })

  it('preserves embedded NUL and Unicode text for Core’s authoritative format validation', async () => {
    const h = harness()
    const path = join(directory, 'embedded-control.md')
    const content = markdown.replace('核验事实并整理证据。', '核验事实\0并整理证据。🙂\n第二行。')
    await writeFile(path, content, 'utf8')
    vi.mocked(h.dependencies.selectImportPath).mockResolvedValue(path)
    await expect(h.importTemplate()).resolves.toEqual({ ok: true, value: imported })
    expect(h.core.requestWorkflows).toHaveBeenCalledExactlyOnceWith({
      operation: 'importTemplateMarkdown',
      markdown: content
    })
  })

  it('rejects renderer-supplied import paths before opening a dialog', async () => {
    const h = harness()
    await expect(h.importTemplate({ path: '/private/secret.md' })).resolves.toEqual(
      failure('organization_template_invalid_request')
    )
    expect(h.dependencies.selectImportPath).not.toHaveBeenCalled()
    expect(h.core.requestWorkflows).not.toHaveBeenCalled()
  })

  it.each([
    ['not-markdown.txt', Buffer.from(markdown)],
    ['invalid.md', Buffer.from([0xc3, 0x28])],
    ['empty.md', Buffer.from('  \n')]
  ])('rejects invalid selected file %s without sending bytes to Core', async (name, bytes) => {
    const h = harness()
    const path = join(directory, name)
    await writeFile(path, bytes)
    vi.mocked(h.dependencies.selectImportPath).mockResolvedValue(path)
    await expect(h.importTemplate()).resolves.toEqual(
      failure('organization_template_invalid_format')
    )
    expect(h.core.requestWorkflows).not.toHaveBeenCalled()
  })

  it('rejects directories even with a Markdown extension', async () => {
    const h = harness()
    const path = join(directory, 'directory.md')
    await mkdir(path)
    vi.mocked(h.dependencies.selectImportPath).mockResolvedValue(path)
    await expect(h.importTemplate()).resolves.toEqual(
      failure('organization_template_invalid_format')
    )
    expect(h.core.requestWorkflows).not.toHaveBeenCalled()
  })

  it.each([WORKFLOW_TEMPLATE_MARKDOWN_MAX_BYTES, WORKFLOW_TEMPLATE_MARKDOWN_MAX_BYTES + 1])(
    'enforces the exact %i-byte boundary before calling Core',
    async (size) => {
      const h = harness()
      const path = join(directory, 'large.md')
      await writeFile(path, Buffer.alloc(size, 97))
      vi.mocked(h.dependencies.selectImportPath).mockResolvedValue(path)
      const result = await h.importTemplate()
      if (size > WORKFLOW_TEMPLATE_MARKDOWN_MAX_BYTES) {
        expect(result).toEqual(failure('organization_template_too_large'))
        expect(h.core.requestWorkflows).not.toHaveBeenCalled()
      } else {
        expect(result).toEqual({ ok: true, value: imported })
        expect(h.core.requestWorkflows.mock.calls[0][0].markdown.length).toBe(size)
      }
    }
  )

  it('reports safe read failures without leaking the selected path', async () => {
    const h = harness()
    vi.mocked(h.dependencies.selectImportPath).mockResolvedValue(
      join(directory, 'private-missing.md')
    )
    await expect(h.importTemplate()).resolves.toEqual(failure('organization_template_read_failed'))
    expect(h.session.dispose).toHaveBeenCalledOnce()
  })

  it.each(['import', 'export'] as const)(
    'preserves structured Core %s failures and writes nothing',
    async (operation) => {
      const h = harness()
      const path = join(directory, 'source.md')
      await writeFile(path, markdown)
      vi.mocked(h.dependencies.selectImportPath).mockResolvedValue(path)
      const error = {
        message: 'organization_template_invalid_format',
        code: -32602,
        data: { code: 'organization_template_invalid_format' }
      }
      h.core.requestWorkflows.mockRejectedValue(Object.assign(new Error(error.message), error))
      await expect(
        operation === 'import' ? h.importTemplate() : h.exportTemplate()
      ).resolves.toEqual({ ok: false, error })
      expect(h.dependencies.selectExportPath).not.toHaveBeenCalled()
      expect(await readdir(directory)).toEqual(['source.md'])
      expect(h.session.dispose).toHaveBeenCalledOnce()
    }
  )

  it.each(['import', 'export'] as const)(
    'projects native %s dialog failures without OS details',
    async (operation) => {
      const h = harness()
      h.core.requestWorkflows.mockResolvedValue(exported)
      vi.mocked(h.dependencies.selectImportPath).mockRejectedValue(
        new Error('/private/platform-details')
      )
      vi.mocked(h.dependencies.selectExportPath).mockRejectedValue(
        new Error('/private/platform-details')
      )
      await expect(
        operation === 'import' ? h.importTemplate() : h.exportTemplate()
      ).resolves.toEqual(failure('organization_template_dialog_failed'))
      expect(h.session.dispose).toHaveBeenCalledOnce()
    }
  )

  it('silently cancels exports after fetching the requested published revision', async () => {
    const h = harness()
    h.core.requestWorkflows.mockResolvedValue(exported)
    await expect(h.exportTemplate()).resolves.toEqual({ ok: true, value: { saved: false } })
    expect(h.core.requestWorkflows).toHaveBeenCalledExactlyOnceWith({
      operation: 'exportTemplateMarkdown',
      ...exportInput
    })
    expect(h.dependencies.selectExportPath).toHaveBeenCalledExactlyOnceWith(event, 'team.md')
    expect(await readdir(directory)).toEqual([])
  })

  it('requires an authoritative exported template before opening a save dialog', async () => {
    const h = harness()
    await expect(h.exportTemplate()).resolves.toEqual(
      failure('organization_template_invalid_format')
    )
    expect(h.dependencies.selectExportPath).not.toHaveBeenCalled()
  })

  it.each([
    null,
    {},
    { ...exportInput, expectedRevision: 0 },
    { ...exportInput, expectedRevision: 1.5 },
    { ...exportInput, path: '/private/secret.md' },
    { ...exportInput, markdown: 'renderer draft' }
  ])('rejects malformed export input %j before Core or native dialogs', async (input) => {
    const h = harness()
    await expect(h.exportTemplate(input)).resolves.toEqual(
      failure('organization_template_invalid_request')
    )
    expect(h.core.requestWorkflows).not.toHaveBeenCalled()
    expect(h.dependencies.selectExportPath).not.toHaveBeenCalled()
  })

  it('atomically replaces the chosen file with Core-produced UTF-8 Markdown', async () => {
    const h = harness()
    const path = join(directory, 'chosen.md')
    await writeFile(path, 'previous content')
    h.core.requestWorkflows.mockResolvedValue(exported)
    vi.mocked(h.dependencies.selectExportPath).mockResolvedValue(path)
    await expect(h.exportTemplate()).resolves.toEqual({ ok: true, value: { saved: true } })
    expect(await readFile(path, 'utf8')).toBe(markdown)
    expect(await readdir(directory)).toEqual(['chosen.md'])
  })

  it('safely reports staging and publication errors and cleans up temporary files', async () => {
    const blockedPath = join(directory, 'blocked.md')
    await mkdir(blockedPath)
    for (const path of [join(directory, 'missing', 'chosen.md'), blockedPath]) {
      const h = harness()
      h.core.requestWorkflows.mockResolvedValue(exported)
      vi.mocked(h.dependencies.selectExportPath).mockResolvedValue(path)
      await expect(h.exportTemplate()).resolves.toEqual(
        failure('organization_template_write_failed')
      )
      expect(await readdir(directory)).toEqual(['blocked.md'])
      expect(h.session.dispose).toHaveBeenCalledOnce()
    }
  })

  it.each(['import', 'export'] as const)(
    'blocks unauthenticated %s requests before native dialogs or Core',
    async (operation) => {
      const h = harness()
      vi.mocked(h.dependencies.beginSession).mockImplementation(() => {
        throw new Error('ACCOUNT_LOGIN_REQUIRED')
      })
      await expect(
        operation === 'import' ? h.importTemplate() : h.exportTemplate()
      ).resolves.toEqual({ ok: false, error: { message: 'ACCOUNT_LOGIN_REQUIRED' } })
      expect(h.core.requestWorkflows).not.toHaveBeenCalled()
      expect(h.dependencies.selectImportPath).not.toHaveBeenCalled()
      expect(h.dependencies.selectExportPath).not.toHaveBeenCalled()
    }
  )

  it('does not import a selected file when its user session ended during the dialog', async () => {
    const h = harness()
    vi.mocked(h.dependencies.selectImportPath).mockImplementation(async () => {
      h.session.assertCurrent.mockImplementation(() => {
        throw new Error('ACCOUNT_LOGIN_REQUIRED')
      })
      return join(directory, 'selected.md')
    })
    await expect(h.importTemplate()).resolves.toEqual({
      ok: false,
      error: { message: 'ACCOUNT_LOGIN_REQUIRED' }
    })
    expect(h.core.requestWorkflows).not.toHaveBeenCalled()
    expect(h.session.dispose).toHaveBeenCalledOnce()
  })

  it('does not publish staged export bytes after the final session fence fails', async () => {
    const h = harness()
    const path = join(directory, 'chosen.md')
    await writeFile(path, 'previous content')
    h.core.requestWorkflows.mockResolvedValue(exported)
    vi.mocked(h.dependencies.selectExportPath).mockResolvedValue(path)
    h.session.assertCurrent
      .mockImplementationOnce(() => undefined)
      .mockImplementationOnce(() => undefined)
      .mockImplementationOnce(() => undefined)
      .mockImplementationOnce(() => {
        throw new Error('ACCOUNT_LOGIN_REQUIRED')
      })
    await expect(h.exportTemplate()).resolves.toEqual({
      ok: false,
      error: { message: 'ACCOUNT_LOGIN_REQUIRED' }
    })
    expect(await readFile(path, 'utf8')).toBe('previous content')
    expect(await readdir(directory)).toEqual(['chosen.md'])
    expect(h.session.dispose).toHaveBeenCalledOnce()
  })
})
