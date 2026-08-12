import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatReadActivity } from '../../features/chat/chatTypes'
import {
  ReadToolActivity,
  ReadToolActivityGroup
} from '../../features/chat/components/toolActivities/ReadToolActivity'
import type { ImageArtifactResolver } from '../../features/imageGeneration/artifacts/ImageArtifactResolver'

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
        filePath: 'preview.png'
      })
      expect(mocks.openImagePreview).toHaveBeenCalledWith({
        alt: 'preview.png',
        fileName: 'preview.png',
        src: 'data:image/png;base64,ZnJlc2g='
      })
    })
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
      expect(artifactResolver.resolve).toHaveBeenCalledWith(artifact)
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
    expect(screen.container.textContent).toContain('目录被误用为文件：crates/mcp-client/src')
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
    expect(screen.container.textContent).toContain(failedPath)
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
