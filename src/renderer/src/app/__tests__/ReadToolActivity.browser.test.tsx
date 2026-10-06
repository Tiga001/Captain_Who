import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { CSSProperties } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { page, userEvent } from 'vitest/browser'
import { frontendConfig, getFrontendCssVariables } from '../../config/frontendConfig'
import { getTranslation } from '../../config/frontendTranslations'
import { classicDarkTheme, classicLightTheme } from '../../config/themes/classic'
import type { ChatReadActivity } from '../../features/chat/chatTypes'
import {
  ReadToolActivity,
  ReadToolActivityGroup
} from '../../features/chat/components/toolActivities/ReadToolActivity'
import type { ImageArtifactResolver } from '../../features/imageGeneration/artifacts/ImageArtifactResolver'
import '../../styles/global.css'
import '../../features/chat/ChatConversationPage.agent.css'
import '../../features/chat/ChatConversationPage.results.css'

const mocks = vi.hoisted(() => ({
  loadImageFile: vi.fn(),
  openImagePreview: vi.fn(),
  showImagePreviewNotice: vi.fn()
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) =>
      ({
        'agent.read.completedWithPathFailures': '读取了 {label}，{count} 个路径失败',
        'agent.read.completedCount': '已读取 {label}',
        'agent.read.count.file': '{count} 个文件',
        'agent.read.count.image': '{count} 张图片',
        'agent.read.failedCount': '{count} 个失败',
        'agent.read.failedReadCount': '读取 {label} 失败',
        'agent.separator': '，',
        'agent.read.file.pathIsDirectory': '目录被误用为文件',
        'agent.read.item': '读取 {fileName}',
        'agent.read.pathIsDirectoryItem': '目录被误用为文件：{path}',
        'agent.read.pathFailedCount': '{count} 个路径失败',
        'agent.activity.read.completed': '已读取',
        'agent.activity.read.running': '正在读取',
        'agent.read.file.failed': getTranslation('zh-CN', 'agent.read.file.failed'),
        'agent.read.word.failed': getTranslation('zh-CN', 'agent.read.word.failed'),
        'agent.read.presentation.failed': getTranslation('zh-CN', 'agent.read.presentation.failed'),
        'agent.read.spreadsheet.failed': getTranslation('zh-CN', 'agent.read.spreadsheet.failed')
      })[key] ?? key
  })
}))

vi.mock('../../features/storage/storageClient', () => ({
  loadImageFile: mocks.loadImageFile
}))

vi.mock('../../features/chat/components/ImagePreview', () => ({
  useImagePreview: () => mocks.openImagePreview,
  useImagePreviewNotice: () => mocks.showImagePreviewNotice
}))

const THUMBNAIL_DATA_URL =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII='
const ARTIFACT_SHA256 = 'a'.repeat(64)
const ARTIFACT_URI = `image-artifact://sha256/${ARTIFACT_SHA256}`

const call: AgentToolCall = {
  id: 'read-image-call',
  tool: 'read_image',
  args: { path: 'preview.png' },
  approvalStatus: 'not_required',
  reason: null
}

function activity(overrides: Partial<ChatReadActivity> = {}): ChatReadActivity {
  return {
    callId: call.id,
    tool: call.tool,
    kind: 'image',
    status: 'completed',
    path: 'preview.png',
    fileName: 'preview.png',
    updatedAt: 1,
    ...overrides
  }
}

describe('ReadToolActivity image presentation', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.loadImageFile.mockResolvedValue({
      name: 'preview.png',
      mimeType: 'image/png',
      sizeBytes: 5,
      data: 'ZnJlc2g='
    })
  })

  it('renders a valid backend thumbnail', async () => {
    const screen = await render(
      <ReadToolActivity activity={activity({ thumbnailDataUrl: THUMBNAIL_DATA_URL })} call={call} />
    )

    expect(screen.container.querySelector('img')?.getAttribute('src')).toBe(THUMBNAIL_DATA_URL)
  })

  it('does not render redaction placeholders or a legacy full-image event payload', async () => {
    const screen = await render(
      <ReadToolActivity
        activity={activity({
          thumbnailDataUrl: 'data:image/png;base64,[binary/base64 omitted]',
          fullDataUrl: 'data:image/png;base64,bGVnYWN5'
        })}
        call={call}
      />
    )

    expect(screen.container.querySelector('img')).toBeNull()
  })

  it('loads the original from its source path instead of using event Base64', async () => {
    const openWorkspaceReference = vi.fn()
    const screen = await render(
      <ReadToolActivity
        assistantMessageId="assistant-1"
        activity={activity({
          thumbnailDataUrl: THUMBNAIL_DATA_URL,
          fullDataUrl: 'data:image/png;base64,bGVnYWN5'
        })}
        call={call}
        conversationId="conversation-1"
        onOpenWorkspaceReference={openWorkspaceReference}
        projectId="project-1"
      />
    )

    const previewButton = screen.container.querySelector<HTMLButtonElement>('.read-activity__image')
    expect(previewButton).not.toBeNull()
    previewButton?.click()
    await vi.waitFor(() => {
      expect(mocks.loadImageFile).toHaveBeenCalledWith({
        projectId: 'project-1',
        filePath: 'preview.png',
        assistantMessageId: 'assistant-1'
      })
      expect(mocks.openImagePreview).toHaveBeenCalledWith({
        alt: 'preview.png',
        fileName: 'preview.png',
        src: 'data:image/png;base64,ZnJlc2g='
      })
      expect(openWorkspaceReference).not.toHaveBeenCalled()
    })
  })

  it('shows an unavailable-file notice when the frozen workspace cannot be resolved', async () => {
    mocks.loadImageFile.mockRejectedValueOnce(new Error('Frozen workspace unavailable'))
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    try {
      const screen = await render(
        <ReadToolActivity
          assistantMessageId="assistant-old"
          activity={activity({ thumbnailDataUrl: THUMBNAIL_DATA_URL })}
          call={call}
          projectId="project-1"
        />
      )
      screen.container.querySelector<HTMLButtonElement>('.read-activity__image')?.click()
      await expect
        .poll(() => mocks.showImagePreviewNotice.mock.calls)
        .toContainEqual(['files.preview.error'])
      expect(mocks.openImagePreview).not.toHaveBeenCalled()
      expect(mocks.loadImageFile).toHaveBeenCalledWith({
        assistantMessageId: 'assistant-old',
        projectId: 'project-1',
        filePath: 'preview.png'
      })
    } finally {
      consoleError.mockRestore()
    }
  })

  it('resolves a generated Artifact through the Host Artifact boundary', async () => {
    const release = vi.fn()
    const retainedRelease = vi.fn()
    const artifactResolver: ImageArtifactResolver = {
      resolve: vi.fn().mockResolvedValue({
        src: 'blob:generated-image',
        release,
        retain: () => retainedRelease
      })
    }
    const artifact = {
      artifactId: `sha256:${ARTIFACT_SHA256}`,
      uri: ARTIFACT_URI,
      kind: 'image' as const,
      format: 'png' as const,
      mimeType: 'image/png',
      width: 1024,
      height: 1024,
      sizeBytes: 2048,
      sha256: ARTIFACT_SHA256
    }
    const result: AgentToolResult = {
      callId: call.id,
      tool: call.tool,
      ok: true,
      result: {
        path: ARTIFACT_URI,
        artifact,
        thumbnailDataUrl: THUMBNAIL_DATA_URL
      }
    }
    const generatedCall: AgentToolCall = {
      ...call,
      args: { path: ARTIFACT_URI }
    }
    const screen = await render(
      <ReadToolActivity
        activity={activity({
          path: ARTIFACT_URI,
          fileName: 'generated.png',
          thumbnailDataUrl: THUMBNAIL_DATA_URL
        })}
        artifactResolver={artifactResolver}
        call={generatedCall}
        result={result}
      />
    )

    screen.container.querySelector<HTMLButtonElement>('.read-activity__image')?.click()
    await vi.waitFor(() => {
      expect(artifactResolver.resolve).toHaveBeenCalledWith(artifact, {})
      expect(mocks.loadImageFile).not.toHaveBeenCalled()
      expect(release).toHaveBeenCalledTimes(1)
      expect(mocks.openImagePreview).toHaveBeenCalledWith({
        alt: 'generated-image-aaaaaaaaaaaa.png',
        fileName: 'generated-image-aaaaaaaaaaaa.png',
        src: 'blob:generated-image',
        release: retainedRelease
      })
    })
  })
})

describe('image read failures layout', () => {
  function imageItem(id: string, failed = false, withError = true) {
    const path =
      id === 'two'
        ? `Playground/很长的截图文件名-${id}-2026-10-06.png`
        : `Playground/截图-${id}.png`
    return {
      call: { ...call, id, args: { path } },
      activity: activity({
        callId: id,
        path,
        fileName: path.split('/').at(-1),
        thumbnailDataUrl: THUMBNAIL_DATA_URL
      }),
      result: {
        callId: id,
        tool: 'read_image',
        ok: !failed,
        ...(failed && withError ? { error: '读取失败：无法解码图片。' } : {})
      }
    }
  }

  it.each([
    { mode: 'light', theme: classicLightTheme },
    { mode: 'dark', theme: classicDarkTheme }
  ])(
    'keeps failed rows separate from thumbnails through updates in $mode panels',
    async ({ mode, theme }) => {
      await page.viewport(1100, 800)
      const successful = ['one', 'two', 'three'].map((id) => imageItem(id))
      const failed = ['one', 'two', 'three'].map((id) => imageItem(id, true))
      const view = (items: typeof successful, width: number) => (
        <div
          style={
            {
              ...getFrontendCssVariables(frontendConfig, theme),
              width,
              padding: 16,
              boxSizing: 'border-box',
              background: 'var(--mc-color-surface-main-panel)',
              fontFamily: 'var(--mc-font-family)'
            } as CSSProperties
          }
        >
          <ReadToolActivityGroup items={items} />
        </div>
      )
      const screen = await render(view(successful, 960))
      screen.container.querySelector('summary')!.click()
      await vi.waitFor(() => expect(screen.container.querySelector('details')!.open).toBe(true))
      expect(screen.container.querySelectorAll('.read-activity__image')).toHaveLength(3)
      const mixed = [
        ...failed,
        ...['retry-one', 'retry-two', 'retry-three'].map((id) => imageItem(id))
      ]
      for (const width of [960, 320]) {
        await screen.rerender(view(mixed, width))
        const list = screen.container.querySelector<HTMLElement>('.read-activity__items')!
        const rows = [...list.querySelectorAll<HTMLElement>('.read-activity__text-item')]
        const images = [...list.querySelectorAll<HTMLElement>('.read-activity__image')]
        expect(rows).toHaveLength(3)
        expect(images).toHaveLength(3)
        expect(list.scrollWidth).toBeLessThanOrEqual(list.clientWidth)
        for (const [index, row] of rows.entries()) {
          expect(row.dataset.status).toBe('failed')
          expect(row.title).toContain(mixed[index].result.error)
          expect(row.getBoundingClientRect().height).toBeLessThanOrEqual(
            parseFloat(getComputedStyle(row).lineHeight) + 1
          )
          // A text row owns a whole flex line: no thumbnail may sit beside it.
          expect(row.getBoundingClientRect().width).toBeGreaterThanOrEqual(list.clientWidth - 4)
        }
        expect(images[0].getBoundingClientRect().top).toBeGreaterThanOrEqual(
          rows[2].getBoundingClientRect().bottom
        )
        for (const image of images) {
          expect(image.getBoundingClientRect().width).toBe(92)
          expect(image.getBoundingClientRect().height).toBe(92)
        }
        expect(images[0].getBoundingClientRect().top).toBe(images[1].getBoundingClientRect().top)
        await page.screenshot({
          element: screen.container.firstElementChild as HTMLElement,
          path: `../../../../../.cache/read-activity/mixed-images-${mode}-${width}.png`
        })
      }
      // A failed tool result is authoritative even if an earlier thumbnail remains and no error text arrived.
      await screen.rerender(view([imageItem('one', true, false), ...failed.slice(1)], 320))
      expect(screen.container.querySelectorAll('.read-activity__image')).toHaveLength(0)
      expect(
        screen.container.querySelectorAll('.read-activity__text-item[data-status="failed"]')
      ).toHaveLength(3)
      // Interleaving preserves timeline order and still gives each failure its own line.
      await screen.rerender(view([successful[0], failed[1], successful[2]], 320))
      const children = [...screen.container.querySelector('.read-activity__items')!.children]
      expect(children[1].getBoundingClientRect().top).toBeGreaterThanOrEqual(
        children[0].getBoundingClientRect().bottom
      )
      expect(children[2].getBoundingClientRect().top).toBeGreaterThanOrEqual(
        children[1].getBoundingClientRect().bottom
      )
    }
  )
})

describe('ReadToolActivity structured path failures', () => {
  function readFileCall(id: string, path: string): AgentToolCall {
    return {
      id,
      tool: 'read_file',
      args: { path },
      approvalStatus: 'not_required',
      reason: null
    }
  }

  function readFileActivity(
    call: AgentToolCall,
    path: string,
    status: ChatReadActivity['status']
  ): ChatReadActivity {
    return {
      callId: call.id,
      tool: call.tool,
      kind: 'file',
      status,
      path,
      fileName: path.split('/').pop() ?? path,
      updatedAt: 1
    }
  }

  function successfulReadResult(call: AgentToolCall, path: string): AgentToolResult {
    return {
      callId: call.id,
      tool: call.tool,
      ok: true,
      result: { path, content: 'ok' }
    }
  }

  function directoryReadResult(call: AgentToolCall, path: string): AgentToolResult {
    return {
      callId: call.id,
      tool: call.tool,
      ok: false,
      result: {
        code: 'path_is_directory',
        errorCode: 'read_file.path_is_directory',
        path,
        message: 'read_file 只能读取普通文本文件。',
        continueWith: {
          tool: 'workspace_map',
          args: { focusPath: path, maxDepth: 3 }
        }
      },
      error: 'read_file 只能读取普通文本文件。'
    }
  }

  it('shows the directory misuse and its complete relative path', async () => {
    const path = 'crates/mcp-client/src'
    const readCall = readFileCall('read-directory', path)
    const openWorkspaceReference = vi.fn()
    const screen = await render(
      <ReadToolActivity
        assistantMessageId="read-message"
        activity={readFileActivity(readCall, path, 'failed')}
        call={readCall}
        conversationId="read-conversation"
        onOpenWorkspaceReference={openWorkspaceReference}
        result={directoryReadResult(readCall, path)}
      />
    )

    expect(screen.container.textContent).toContain('目录被误用为文件')
    const row = screen.container.querySelector<HTMLElement>('.read-activity__text-item')!
    expect(row.title).toContain(path)
    expect(row.querySelector('.read-activity__file-name')?.textContent).toBe('src')
    expect(row.querySelector('.read-activity__directory')).toBeNull()
    expect(row.dataset.status).toBe('failed')
    expect(row.querySelector('button')).toBeNull()
    expect(openWorkspaceReference).not.toHaveBeenCalled()
  })

  it('separates successful files from path failures in a group', async () => {
    const successful = ['crates/mcp-client/src/lib.rs', 'crates/core/src/lib.rs', 'README.md'].map(
      (path, index) => {
        const readCall = readFileCall(`read-success-${index}`, path)
        return {
          activity: readFileActivity(readCall, path, 'completed'),
          call: readCall,
          result: successfulReadResult(readCall, path)
        }
      }
    )
    const failedPath = 'crates/mcp-client/src'
    const failedCall = readFileCall('read-directory', failedPath)
    const screen = await render(
      <ReadToolActivityGroup
        items={[
          ...successful,
          {
            activity: readFileActivity(failedCall, failedPath, 'failed'),
            call: failedCall,
            result: directoryReadResult(failedCall, failedPath)
          }
        ]}
      />
    )

    expect(screen.container.textContent).toContain('读取了 3 个文件，1 个路径失败')
    expect(screen.container.querySelector<HTMLElement>('[data-status="failed"]')?.title).toContain(
      failedPath
    )
  })

  it('keeps ordinary read failures on the generic failure label', async () => {
    const path = 'crates/core/src/missing.rs'
    const readCall = readFileCall('read-missing', path)
    const result: AgentToolResult = {
      callId: readCall.id,
      tool: readCall.tool,
      ok: false,
      error: '文件不存在'
    }
    const screen = await render(
      <ReadToolActivity
        activity={readFileActivity(readCall, path, 'failed')}
        call={readCall}
        result={result}
      />
    )

    expect(screen.container.textContent).toContain(
      getTranslation('zh-CN', 'agent.read.file.failed')
    )
    expect(screen.container.textContent).not.toContain('目录被误用为文件')
  })
})

describe('standalone read summary layout', () => {
  const fixtures = [
    { tool: 'read_file', name: 'TEAM.md' },
    { tool: 'read_word', name: '研究报告.docx' },
    { tool: 'read_presentation', name: 'project-review.pptx' },
    {
      tool: 'read_spreadsheet',
      name: 'bimolecular-termination-kinetics-validation-and-unit-consistency-review.xlsx'
    }
  ].map(({ tool, name }) => ({
    id: `standalone-${tool}`,
    tool,
    args: { path: `/Users/example/Documents/External reference reports/${name}` },
    approvalStatus: 'not_required' as const,
    reason: null
  }))

  it.each([
    { mode: 'light', theme: classicLightTheme },
    { mode: 'dark', theme: classicDarkTheme }
  ])(
    'keeps icons and filenames together through state and width changes in $mode',
    async ({ mode, theme }) => {
      await page.viewport(1000, 700)
      const onOpenWorkspaceReference = vi.fn()
      const view = (width: number, status: 'running' | 'completed' | 'failed') => (
        <div
          style={
            {
              ...getFrontendCssVariables(frontendConfig, theme),
              width,
              padding: 16,
              display: 'grid',
              gap: 8,
              background: 'var(--mc-color-surface-main-panel)',
              fontFamily: 'var(--mc-font-family)'
            } as CSSProperties
          }
        >
          {fixtures.map((readCall) => (
            <ReadToolActivity
              assistantMessageId="read-message"
              call={readCall}
              conversationId="read-conversation"
              key={readCall.id}
              onOpenWorkspaceReference={onOpenWorkspaceReference}
              result={
                status === 'running'
                  ? undefined
                  : {
                      callId: readCall.id,
                      tool: readCall.tool,
                      ok: status === 'completed',
                      result: { path: readCall.args.path },
                      ...(status === 'failed' ? { error: '文件暂时不可读取' } : {})
                    }
              }
            />
          ))}
        </div>
      )
      const screen = await render(view(736, 'running'))
      for (const width of [736, 320]) {
        for (const status of ['running', 'completed', 'failed'] as const) {
          await screen.rerender(view(width, status))
          const panel = screen.container.firstElementChild as HTMLElement
          const summaries = [
            ...panel.querySelectorAll<HTMLElement>(
              '.agent-activity--read > .agent-activity__static-summary, .agent-activity--read > summary'
            )
          ]
          expect(summaries).toHaveLength(fixtures.length)
          for (const [index, summary] of summaries.entries()) {
            const icon = summary.querySelector<HTMLElement>('.agent-activity__icon')!
            const action = summary.querySelector<HTMLElement>('.read-activity__action')!
            const filename = summary.querySelector<HTMLElement>('.read-activity__file-name')!
            const iconRect = icon.getBoundingClientRect()
            const actionRect = action.getBoundingClientRect()
            const filenameRect = filename.getBoundingClientRect()
            // Check the entire summary: a one-line filename alone misses the icon stranded above it.
            expect(
              Math.abs(iconRect.top + iconRect.height / 2 - actionRect.top - actionRect.height / 2)
            ).toBeLessThanOrEqual(3)
            expect(
              Math.abs(
                filenameRect.top + filenameRect.height / 2 - actionRect.top - actionRect.height / 2
              )
            ).toBeLessThanOrEqual(3)
            expect(iconRect.right).toBeLessThanOrEqual(actionRect.left)
            expect(actionRect.right).toBeLessThanOrEqual(filenameRect.left)
            expect(filenameRect.width).toBeGreaterThan(0)
            expect(summary.getBoundingClientRect().height).toBeLessThanOrEqual(
              Math.max(
                parseFloat(getComputedStyle(summary).minHeight) || 0,
                iconRect.height,
                actionRect.height,
                filenameRect.height
              ) + 1
            )
            expect(summary.scrollWidth).toBeLessThanOrEqual(summary.clientWidth + 1)
            expect(filename.getAttribute('aria-label')).toBe(fixtures[index].args.path)
            expect(getComputedStyle(filename).textOverflow).toBe('ellipsis')
            expect(summary.querySelector('.read-activity__directory')).toBeNull()
            expect(action.textContent).toContain(status === 'failed' ? '失败' : '读取')
            if (width === 320 && index === fixtures.length - 1) {
              expect(filename.scrollWidth).toBeGreaterThan(filename.clientWidth)
            }
          }
          expect(panel.scrollWidth).toBeLessThanOrEqual(panel.clientWidth + 1)
          expect(panel.querySelectorAll('details')).toHaveLength(
            status === 'failed' ? fixtures.length : 0
          )
          await page.screenshot({
            element: panel,
            path: `../../../../../.cache/read-summary-fix/${mode}-${width}-${status}.png`
          })
        }
      }
    }
  )
})

describe('compact read file rows', () => {
  const directory = '03_Cases/C001_FRP_Biomolecular_Termination_CSTR'
  const names = ['mechanism.md', 'mechanism.md', 'conclusions.md', 'applicability_assessment.md']
  const items = names.map((name, index) => ({
    call: {
      id: `compact-read-${index}`,
      tool: 'read_file',
      args: { path: `${directory}/${name}` },
      approvalStatus: 'not_required' as const,
      reason: null
    },
    result: {
      callId: `compact-read-${index}`,
      tool: 'read_file',
      ok: true,
      result: { path: `${directory}/${name}` }
    }
  }))

  it('navigates same-named files with their authoritative path and turn identity by mouse and keyboard', async () => {
    const openWorkspaceReference = vi.fn()
    const paths = [
      'packages/core/index.ts',
      'packages/ui/index.ts',
      '/Users/example/.mycopilot/attachments/message-1/report.md',
      'C:\\work\\outside\\report.md'
    ]
    const screen = await render(
      <div style={getFrontendCssVariables() as CSSProperties}>
        {paths.map((path, index) => (
          <ReadToolActivity
            assistantMessageId="read-message"
            activity={activity({
              callId: `link-${index}`,
              tool: 'read_file',
              kind: 'file',
              path: 'stale/event/path.txt',
              fileName: 'stale.txt'
            })}
            call={{ ...items[0].call, id: `link-${index}`, args: { path: 'requested/path.txt' } }}
            conversationId="read-conversation"
            key={path}
            onOpenWorkspaceReference={openWorkspaceReference}
            presentation="compact"
            projectId="read-project"
            result={{ ...items[0].result, callId: `link-${index}`, result: { path } }}
          />
        ))}
      </div>
    )
    const buttons = paths.map((path) => screen.getByRole('button', { name: path, exact: true }))
    expect(buttons[0].element().textContent).toBe('index.ts')
    expect(buttons[1].element().textContent).toBe('index.ts')
    expect(screen.container.querySelector('.read-activity__directory')).toBeNull()
    await userEvent.click(buttons[0])
    ;(buttons[1].element() as HTMLButtonElement).focus()
    await userEvent.keyboard('{Enter}')
    ;(buttons[2].element() as HTMLButtonElement).focus()
    await userEvent.keyboard(' ')
    await userEvent.click(buttons[3])
    expect(openWorkspaceReference.mock.calls).toEqual(
      paths.map((filePath, index) => [
        {
          source: 'read-tool',
          filePath,
          assistantMessageId: 'read-message',
          conversationId: 'read-conversation',
          callId: `link-${index}`,
          projectId: 'read-project'
        }
      ])
    )

    await userEvent.hover(buttons[0])
    await expect.element(page.getByRole('tooltip')).toHaveTextContent(paths[0])
    expect(page.getByRole('tooltip').element().getAttribute('data-appearance')).toBe('inverse')
  })

  it('opens a file from an error summary without changing its disclosure state', async () => {
    const openWorkspaceReference = vi.fn()
    const expanded = vi.fn()
    const path = 'reports/missing.md'
    const screen = await render(
      <div style={getFrontendCssVariables() as CSSProperties}>
        <ReadToolActivity
          assistantMessageId="read-message"
          call={{ ...items[0].call, args: { path } }}
          conversationId="read-conversation"
          onExpandedChange={expanded}
          onOpenWorkspaceReference={openWorkspaceReference}
          presentation="compact"
          projectId="read-project"
          result={{ ...items[0].result, ok: false, result: { path }, error: '文件暂时不可读取' }}
        />
      </div>
    )
    const details = screen.container.querySelector('details')!
    const link = screen.getByRole('button', { name: path, exact: true })
    expect(details.open).toBe(false)
    await userEvent.click(link)
    ;(link.element() as HTMLButtonElement).focus()
    await userEvent.keyboard('{Enter}')
    await userEvent.keyboard(' ')
    expect(details.open).toBe(false)
    expect(expanded).not.toHaveBeenCalled()
    expect(openWorkspaceReference).toHaveBeenCalledTimes(3)
    await userEvent.click(screen.container.querySelector('.read-activity__action')!)
    await expect.poll(() => details.open).toBe(true)
    expect(expanded).toHaveBeenCalledWith(true)
    await userEvent.click(link)
    expect(details.open).toBe(true)
    expect(screen.getByText('文件暂时不可读取')).toBeVisible()
  })

  it('ellipsizes a long basename while preserving the full-path hover and exact external target', async () => {
    const name = 'bimolecular-termination-kinetics-validation-and-unit-consistency-review.md'
    const path = `/Users/example/Documents/External reference reports/${name}`
    const openWorkspaceReference = vi.fn()
    const screen = await render(
      <div style={{ ...getFrontendCssVariables(), width: 320, padding: 16 } as CSSProperties}>
        <ReadToolActivity
          assistantMessageId="read-message"
          call={{ ...items[0].call, args: { path } }}
          conversationId="read-conversation"
          onOpenWorkspaceReference={openWorkspaceReference}
          presentation="compact"
          result={{ ...items[0].result, result: { path } }}
        />
      </div>
    )
    const row = screen.container.querySelector<HTMLElement>('.read-activity__text-item')!
    const button = screen.getByRole('button', { name: path, exact: true })
    expect(button.element().textContent).toBe(name)
    expect(row.scrollWidth).toBeLessThanOrEqual(row.clientWidth)
    expect(button.element().scrollWidth).toBeGreaterThan(button.element().clientWidth)
    expect(getComputedStyle(button.element()).textOverflow).toBe('ellipsis')
    await userEvent.hover(button)
    await expect.element(page.getByRole('tooltip')).toHaveTextContent(path)
    await userEvent.click(button)
    expect(openWorkspaceReference).toHaveBeenCalledExactlyOnceWith({
      source: 'read-tool',
      filePath: path,
      assistantMessageId: 'read-message',
      conversationId: 'read-conversation',
      callId: items[0].call.id
    })
  })

  it.each([
    { mode: 'light', theme: classicLightTheme },
    { mode: 'dark', theme: classicDarkTheme }
  ])('keeps long paths on one line in wide and narrow $mode panels', async ({ mode, theme }) => {
    await page.viewport(1100, 700)
    const openWorkspaceReference = vi.fn()
    const view = (width: number) => (
      <div
        style={
          {
            ...getFrontendCssVariables(frontendConfig, theme),
            width,
            padding: 16,
            background: 'var(--mc-color-surface-main-panel)',
            fontFamily: 'var(--mc-font-family)'
          } as CSSProperties
        }
      >
        <ReadToolActivityGroup
          assistantMessageId="read-message"
          conversationId="read-conversation"
          items={items}
          onOpenWorkspaceReference={openWorkspaceReference}
          projectId="read-project"
        />
      </div>
    )
    const screen = await render(view(960))
    await screen.getByText('已读取 4 个文件').click()
    const chevron = screen.container.querySelector('.agent-activity__chevron')!
    await expect.poll(() => getComputedStyle(chevron).transform).toBe('matrix(1, 0, 0, 1, 0, 0)')
    for (const width of [960, 320]) {
      await screen.rerender(view(width))
      const rows = [...screen.container.querySelectorAll<HTMLElement>('.read-activity__text-item')]
      expect(rows).toHaveLength(4)
      expect(
        rows.map((row) => row.querySelector('.read-activity__file-name')?.textContent)
      ).toEqual(names)
      for (const [index, row] of rows.entries()) {
        expect(row.title).toBe(`${directory}/${names[index]}`)
        expect(row.getBoundingClientRect().height).toBeLessThanOrEqual(
          parseFloat(getComputedStyle(row).lineHeight) + 1
        )
        expect(row.scrollWidth).toBeLessThanOrEqual(row.clientWidth)
        const name = row.querySelector<HTMLElement>('.read-activity__file-name')!
        expect(name.scrollWidth).toBeLessThanOrEqual(name.clientWidth)
        expect(name.tagName).toBe('BUTTON')
        expect(getComputedStyle(name).textDecorationLine).toBe('underline')
        expect(getComputedStyle(name).backgroundColor).toBe('rgba(0, 0, 0, 0)')
        expect(row.querySelector('.read-activity__directory')).toBeNull()
        expect(row.textContent).not.toContain(directory)
      }
      const details = screen.container.querySelector<HTMLElement>('.read-activity__details')!
      if (width === 960) expect(details.getBoundingClientRect().width).toBeGreaterThan(420)
      await page.screenshot({
        element: screen.container.firstElementChild as HTMLElement,
        path: `../../../../../.cache/read-file-links/${mode}-${width}.png`
      })
    }
  })

  it('preserves authoritative paths, filenames without directories and Windows path tooltips', async () => {
    const screen = await render(
      <ReadToolActivityGroup
        items={[
          { ...items[0], result: { ...items[0].result, result: { path: 'actual/result.md' } } },
          { call: { ...items[1].call, args: { path: 'README.md' } } },
          { call: { ...items[2].call, args: { path: 'C:\\project\\报告.md' } } }
        ]}
      />
    )
    const rows = [...screen.container.querySelectorAll<HTMLElement>('.read-activity__text-item')]
    expect(rows[0].title).toBe('actual/result.md')
    expect(rows[0].querySelector('.read-activity__file-name')?.textContent).toBe('result.md')
    expect(rows[0].querySelector('.read-activity__directory')).toBeNull()
    expect(rows[1].querySelector('.read-activity__file-name')?.textContent).toBe('README.md')
    expect(rows[1].querySelector('.read-activity__directory')).toBeNull()
    expect(rows[2].title).toBe('C:\\project\\报告.md')
    expect(rows[2].querySelector('.read-activity__file-name')?.textContent).toBe('报告.md')
    expect(rows[2].querySelector('.read-activity__directory')).toBeNull()
  })
})
