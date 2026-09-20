import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatMessage } from '../chatTypes'
import { ChatMessageItem } from '../components/ChatMessageItem'

const mocks = vi.hoisted(() => ({
  thumbnail: vi.fn(),
  display: vi.fn(),
  storedImage: vi.fn(),
  open: vi.fn(),
  notice: vi.fn()
}))
vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: vi.fn() })
}))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => mocks.open,
  useImagePreviewNotice: () => mocks.notice
}))
vi.mock('../../storage/storageClient', () => ({
  loadAttachmentImage: mocks.storedImage,
  loadImageFile: vi.fn(),
  revealStoredProjectFile: vi.fn()
}))
vi.mock('../chatAttachments', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../chatAttachments')>()),
  loadComposerAttachmentPreview: mocks.thumbnail,
  loadComposerAttachmentImage: mocks.display
}))

const imageData =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII='
const imageUrl = `data:image/png;base64,${imageData}`
const managedAttachment = {
  id: 'image-1',
  name: 'photo.png',
  kind: 'image' as const,
  mimeType: 'image/png',
  sizeBytes: 100_000_000,
  encoding: 'managed' as const,
  data: 'managed-token'
}
function message(attachments: ChatMessage['attachments'] = [managedAttachment]): ChatMessage {
  return {
    id: 'user-1',
    role: 'user',
    content: 'Look at the photo',
    createdAt: 1,
    status: 'pending',
    attachments
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  mocks.thumbnail.mockResolvedValue(imageUrl)
  mocks.display.mockResolvedValue(imageUrl)
  mocks.storedImage.mockResolvedValue({ name: 'photo.png', mimeType: 'image/png', data: imageData })
})

describe('Managed message attachments', () => {
  it('renders committed files as compact shared cards without a duplicate text summary', async () => {
    const fileAttachment = {
      ...managedAttachment,
      id: 'file-1',
      kind: 'file' as const,
      name: 'report.pdf',
      mimeType: 'application/pdf'
    }
    const screen = await render(
      <ChatMessageItem message={message([fileAttachment])} showTokenUsageDetails={false} />
    )
    const card = screen.container.querySelector<HTMLElement>('.chat-message-attachment')
    expect(card).toHaveClass('attachment-card')
    expect(card?.dataset.kind).toBe('file')
    expect(card?.querySelector('.composer-attachment__remove')).toBeNull()
    expect(getComputedStyle(card!).height).toBe('28px')
    expect(screen.container.textContent).not.toContain('Attachments: report.pdf')
  })

  it('shows an optimistic managed thumbnail, loads its bounded viewer on demand, and retains the thumbnail after persistence', async () => {
    const original = message()
    const screen = await render(
      <ChatMessageItem message={original} showTokenUsageDetails={false} />
    )
    await expect
      .element(screen.getByRole('img', { name: 'photo.png' }))
      .toHaveAttribute('src', imageUrl)
    expect(mocks.thumbnail).toHaveBeenCalledWith(managedAttachment)
    expect(original.attachments?.[0]).toEqual(managedAttachment)
    expect(original.attachments?.[0].previewData).toBeUndefined()
    await screen.getByRole('button', { name: 'photo.png', exact: true }).click()
    expect(mocks.display).toHaveBeenCalledWith(managedAttachment)
    expect(mocks.open).toHaveBeenCalledWith({
      alt: 'photo.png',
      fileName: 'photo.png',
      src: imageUrl
    })
    expect(mocks.storedImage).not.toHaveBeenCalled()

    const persisted = { ...managedAttachment, encoding: undefined, data: undefined }
    await screen.rerender(
      <ChatMessageItem message={message([persisted])} showTokenUsageDetails={false} />
    )
    await expect
      .element(screen.getByRole('img', { name: 'photo.png' }))
      .toHaveAttribute('src', imageUrl)
    await screen.getByRole('button', { name: 'photo.png', exact: true }).click()
    expect(mocks.storedImage).toHaveBeenCalledWith('image-1')
    expect(mocks.thumbnail).toHaveBeenCalledTimes(1)
  })

  it('continues loading persisted images by attachment ID without inventing managed tokens', async () => {
    const persisted = {
      ...managedAttachment,
      encoding: undefined,
      data: undefined,
      previewData: imageData,
      previewMimeType: 'image/png'
    }
    const screen = await render(
      <ChatMessageItem message={message([persisted])} showTokenUsageDetails={false} />
    )
    await screen.getByRole('button', { name: 'photo.png', exact: true }).click()
    expect(mocks.storedImage).toHaveBeenCalledWith('image-1')
    expect(mocks.thumbnail).not.toHaveBeenCalled()
    expect(mocks.display).not.toHaveBeenCalled()
    expect(mocks.open).toHaveBeenCalledWith({
      alt: 'photo.png',
      fileName: 'photo.png',
      src: imageUrl
    })
  })
})
