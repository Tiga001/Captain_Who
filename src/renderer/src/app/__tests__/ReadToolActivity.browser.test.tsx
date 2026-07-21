import type { AgentToolCall } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatReadActivity } from '../../features/chat/chatTypes'
import { ReadToolActivity } from '../../features/chat/components/toolActivities/ReadToolActivity'

const mocks = vi.hoisted(() => ({
  loadImageFile: vi.fn(),
  openImagePreview: vi.fn(),
  showImagePreviewNotice: vi.fn()
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
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

const call: AgentToolCall = {
  id: 'read-image-call',
  tool: 'read_image',
  args: { path: 'preview.png' },
  approvalStatus: 'not_required'
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
})
