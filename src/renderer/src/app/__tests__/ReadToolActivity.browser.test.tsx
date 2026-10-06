import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { CSSProperties } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { page } from 'vitest/browser'
import { frontendConfig, getFrontendCssVariables } from '../../config/frontendConfig'
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
        'agent.read.file.pathIsDirectory': '目录被误用为文件',
        'agent.read.item': '读取 {fileName}',
        'agent.read.pathIsDirectoryItem': '目录被误用为文件：{path}',
        'agent.read.pathFailedCount': '{count} 个路径失败'
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
    const screen = await render(
      <ReadToolActivity
        assistantMessageId="assistant-1"
        activity={activity({
          thumbnailDataUrl: THUMBNAIL_DATA_URL,
          fullDataUrl: 'data:image/png;base64,bGVnYWN5'
        })}
        call={call}
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
    const screen = await render(
      <ReadToolActivity
        activity={readFileActivity(readCall, path, 'failed')}
        call={readCall}
        result={directoryReadResult(readCall, path)}
      />
    )

    expect(screen.container.textContent).toContain('目录被误用为文件')
    const row = screen.container.querySelector<HTMLElement>('.read-activity__text-item')!
    expect(row.title).toContain(path)
    expect(row.querySelector('.read-activity__file-name')?.textContent).toBe('src')
    expect(row.querySelector('.read-activity__directory')?.textContent).toBe('crates/mcp-client')
    expect(row.dataset.status).toBe('failed')
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

    expect(screen.container.textContent).toContain('agent.read.file.failed')
    expect(screen.container.textContent).not.toContain('目录被误用为文件')
  })
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

  it.each([
    { mode: 'light', theme: classicLightTheme },
    { mode: 'dark', theme: classicDarkTheme }
  ])('keeps long paths on one line in wide and narrow $mode panels', async ({ mode, theme }) => {
    await page.viewport(1100, 700)
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
        <ReadToolActivityGroup items={items} />
      </div>
    )
    const screen = await render(view(960))
    await screen.getByText('已读取 4 个文件').click()
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
        const path = row.querySelector<HTMLElement>('.read-activity__directory')!
        expect(path.textContent).toBe(directory)
        expect(getComputedStyle(path).color).not.toBe(getComputedStyle(name).color)
        if (width === 320) expect(path.scrollWidth).toBeGreaterThan(path.clientWidth)
      }
      const details = screen.container.querySelector<HTMLElement>('.read-activity__details')!
      if (width === 960) expect(details.getBoundingClientRect().width).toBeGreaterThan(420)
      await page.screenshot({
        element: screen.container.firstElementChild as HTMLElement,
        path: `../../../../../.cache/read-activity/${mode}-${width}.png`
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
    expect(rows[0].querySelector('.read-activity__directory')?.textContent).toBe('actual')
    expect(rows[1].querySelector('.read-activity__file-name')?.textContent).toBe('README.md')
    expect(rows[1].querySelector('.read-activity__directory')).toBeNull()
    expect(rows[2].title).toBe('C:\\project\\报告.md')
    expect(rows[2].querySelector('.read-activity__file-name')?.textContent).toBe('报告.md')
    expect(rows[2].querySelector('.read-activity__directory')?.textContent).toBe('C:/project')
  })
})
