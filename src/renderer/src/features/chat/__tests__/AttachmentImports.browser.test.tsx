import { useState } from 'react'
import type { AgentInputAttachment, AttachmentImportProgress } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { AttachmentImportOptions, ComposerAttachment } from '../chatAttachments'
import { ComposerAttachments } from '../components/ComposerAttachments'
import { useAttachmentImports, useComposerAttachmentPreviews } from '../useAttachmentImports'
import '../../../styles/global.css'
import '../components/ChatComposer.css'

const mocks = vi.hoisted(() => ({
  create: vi.fn(),
  select: vi.fn(),
  retry: vi.fn(),
  cancel: vi.fn(async () => {}),
  preview: vi.fn(),
  accepted: vi.fn(),
  errors: vi.fn(),
  listeners: new Set<(event: AttachmentImportProgress) => void>()
}))

vi.mock('../../../host/hostClient', () => ({
  hostClient: {
    attachments: {
      cancelImport: mocks.cancel,
      retryInputAttachment: mocks.retry,
      onImportProgress: (listener: (event: AttachmentImportProgress) => void) => {
        mocks.listeners.add(listener)
        return () => mocks.listeners.delete(listener)
      }
    }
  }
}))

vi.mock('../chatAttachments', () => ({
  createComposerAttachmentsFromFiles: mocks.create,
  selectComposerAttachments: mocks.select,
  loadComposerAttachmentPreview: mocks.preview,
  composerAttachmentFromAgentAttachment: (attachment: AgentInputAttachment) => ({
    ...attachment,
    agentAttachment: attachment
  })
}))

function attachment(id = 'attachment-1', name = 'large.txt'): ComposerAttachment {
  const agentAttachment: AgentInputAttachment = {
    id,
    name,
    kind: 'file',
    sizeBytes: 8,
    encoding: 'managed',
    data: 'managed-test-dGVzdGRhdGE='
  }
  return { ...agentAttachment, agentAttachment }
}

function Harness({ scope = 'first' }: { scope?: string }) {
  const [attachments, setAttachments] = useState<ComposerAttachment[]>([])
  const imports = useAttachmentImports({
    scope,
    onAttachments: (values) => {
      mocks.accepted(values)
      setAttachments((previous) => [...previous, ...values])
    },
    onError: mocks.errors,
    errorMessage: () => 'Import failed'
  })
  const previews = useComposerAttachmentPreviews(attachments)
  return (
    <div style={{ width: 640 }}>
      <button
        onClick={() =>
          void imports.addFiles([new File(['testdata'], 'large.txt', { type: 'text/plain' })])
        }
      >
        Add
      </button>
      <button onClick={() => void imports.select('file')}>Choose</button>
      <button
        onClick={() => {
          imports.cancelAll()
          setAttachments([attachment('queued-file', 'queued.txt')])
        }}
      >
        Edit queued message
      </button>
      <button disabled={imports.hasPending}>Send</button>
      <output>{attachments.map((item) => item.id).join(',')}</output>
      <ComposerAttachments
        attachments={[...previews, ...imports.pending]}
        label="Attachments"
        removeLabel="Remove"
        cancelLabel="Cancel"
        retryLabel="Retry"
        onRemove={imports.cancel}
        onRetry={(id) => void imports.retry(id)}
        onPreview={vi.fn()}
      />
    </div>
  )
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((accept, fail) => {
    resolve = accept
    reject = fail
  })
  return { promise, resolve, reject }
}

function progress(event: AttachmentImportProgress) {
  for (const listener of mocks.listeners) listener(event)
}

beforeEach(() => {
  vi.clearAllMocks()
  mocks.create.mockReset()
  mocks.select.mockReset()
  mocks.retry.mockReset()
  mocks.preview.mockResolvedValue(undefined)
})

describe('Attachment imports', () => {
  it.each(['browser', 'native'] as const)(
    'does not append an old %s import after a queued message replaces the draft',
    async (source) => {
      const pending = deferred<ComposerAttachment[]>()
      mocks.create.mockReturnValue(pending.promise)
      mocks.select.mockReturnValue(pending.promise)
      const screen = await render(<Harness />)
      await screen
        .getByRole('button', { name: source === 'browser' ? 'Add' : 'Choose', exact: true })
        .click()
      await screen.getByRole('button', { name: 'Edit queued message', exact: true }).click()
      if (source === 'browser') {
        const options = mocks.create.mock.calls[0][1] as AttachmentImportOptions
        expect(options.signal?.aborted).toBe(true)
        pending.resolve([attachment(options.id)])
      } else {
        const requestId = mocks.select.mock.calls[0][1] as string
        expect(mocks.cancel).toHaveBeenCalledWith({ importId: requestId })
        pending.resolve([attachment()])
      }
      await expect.element(screen.getByRole('button', { name: 'Send', exact: true })).toBeEnabled()
      expect(mocks.accepted).not.toHaveBeenCalled()
      expect(screen.container.querySelector('output')?.textContent).toBe('queued-file')
    }
  )
  it('shows progress, retains a failed card for retry, and only enables sending after acceptance', async () => {
    const first = deferred<ComposerAttachment[]>()
    const second = deferred<ComposerAttachment[]>()
    mocks.create
      .mockImplementationOnce((_files: File[], options: AttachmentImportOptions) => {
        options.onProgress?.({
          attachment: attachment(options.id).agentAttachment,
          receivedBytes: 4,
          status: 'importing'
        })
        return first.promise
      })
      .mockImplementationOnce(() => second.promise)
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'Add', exact: true }).click()
    await expect
      .element(screen.getByRole('progressbar', { name: 'large.txt' }))
      .toHaveAttribute('aria-valuenow', '50')
    await expect.element(screen.getByRole('button', { name: 'Send', exact: true })).toBeDisabled()
    first.reject(new Error('disk full'))
    await expect
      .element(screen.getByRole('button', { name: 'Retry large.txt', exact: true }))
      .toBeVisible()
    expect(mocks.accepted).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'Retry large.txt', exact: true }).click()
    const id = mocks.create.mock.calls[0][1].id as string
    expect(mocks.create.mock.calls[1][1].id).toBe(id)
    second.resolve([attachment(id)])
    await expect.element(screen.getByRole('button', { name: 'Send', exact: true })).toBeEnabled()
    expect(mocks.accepted).toHaveBeenCalledExactlyOnceWith([attachment(id)])
    expect(screen.container.querySelector('[role="progressbar"]')).toBeNull()
  })

  it('aborts cancelled browser imports and ignores their late result', async () => {
    const pending = deferred<ComposerAttachment[]>()
    mocks.create.mockReturnValue(pending.promise)
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'Add', exact: true }).click()
    const options = mocks.create.mock.calls[0][1] as AttachmentImportOptions
    await screen.getByRole('button', { name: 'Cancel large.txt', exact: true }).click()
    expect(options.signal?.aborted).toBe(true)
    pending.resolve([attachment(options.id)])
    await expect.element(screen.getByRole('button', { name: 'Send', exact: true })).toBeEnabled()
    expect(mocks.accepted).not.toHaveBeenCalled()
    expect(screen.container.querySelector('.composer-attachment')).toBeNull()
  })

  it('cancels native progress from an old draft even after returning to that same conversation', async () => {
    const pending = deferred<ComposerAttachment[]>()
    mocks.select.mockReturnValue(pending.promise)
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'Choose', exact: true }).click()
    const requestId = mocks.select.mock.calls[0][1] as string
    await screen.rerender(<Harness scope="second" />)
    await screen.rerender(<Harness scope="first" />)
    progress({
      requestId,
      attachment: attachment().agentAttachment,
      receivedBytes: 4,
      status: 'importing'
    })
    expect(mocks.cancel).toHaveBeenCalledWith({ importId: 'attachment-1' })
    pending.resolve([attachment()])
    await expect.element(screen.getByRole('button', { name: 'Send', exact: true })).toBeEnabled()
    expect(mocks.accepted).not.toHaveBeenCalled()
    expect(screen.container.querySelector('.composer-attachment')).toBeNull()
  })

  it('aborts browser imports on a draft switch and never appends their result to the new draft', async () => {
    const pending = deferred<ComposerAttachment[]>()
    mocks.create.mockReturnValue(pending.promise)
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'Add', exact: true }).click()
    const options = mocks.create.mock.calls[0][1] as AttachmentImportOptions
    await screen.rerender(<Harness scope="second" />)
    expect(options.signal?.aborted).toBe(true)
    pending.resolve([attachment(options.id)])
    await expect.element(screen.getByRole('button', { name: 'Send', exact: true })).toBeEnabled()
    expect(mocks.accepted).not.toHaveBeenCalled()
    expect(screen.container.querySelector('.composer-attachment')).toBeNull()
  })

  it('loads managed image previews without mutating the attachment saved in the draft', async () => {
    const image = attachment('image', 'image.png')
    image.kind = 'image'
    image.agentAttachment = {
      ...image.agentAttachment,
      kind: 'image',
      encoding: 'managed',
      data: 'managed-token'
    }
    mocks.select.mockResolvedValue([image])
    const imageUrl = 'data:image/png;base64,preview'
    mocks.preview.mockResolvedValue(imageUrl)
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'Choose', exact: true }).click()
    await expect
      .element(screen.getByRole('img', { name: 'image.png' }))
      .toHaveAttribute('src', imageUrl)
    expect(mocks.preview).toHaveBeenCalledWith(image.agentAttachment)
    expect(mocks.accepted.mock.calls[0][0][0].previewUrl).toBeUndefined()
    expect(mocks.accepted.mock.calls[0][0][0].agentAttachment.data).toBe('managed-token')
  })

  it('retains native failure details and retries only its selected file', async () => {
    const pending = deferred<ComposerAttachment[]>()
    mocks.select.mockReturnValue(pending.promise)
    mocks.retry.mockResolvedValue(attachment().agentAttachment)
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'Choose', exact: true }).click()
    const requestId = mocks.select.mock.calls[0][1] as string
    progress({
      requestId,
      attachment: attachment().agentAttachment,
      receivedBytes: 2,
      status: 'failed',
      error: 'disk full'
    })
    pending.resolve([])
    await expect
      .element(screen.getByRole('button', { name: 'Retry large.txt', exact: true }))
      .toBeVisible()
    await screen.getByRole('button', { name: 'Retry large.txt', exact: true }).click()
    expect(mocks.retry).toHaveBeenCalledWith({ attachmentId: 'attachment-1' })
    await expect.element(screen.getByRole('button', { name: 'Send', exact: true })).toBeEnabled()
    expect(mocks.accepted).toHaveBeenCalledExactlyOnceWith([attachment()])
  })

  it('updates native file metadata after stat so its progress uses the real size', async () => {
    const pending = deferred<ComposerAttachment[]>()
    mocks.select.mockReturnValue(pending.promise)
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'Choose', exact: true }).click()
    const requestId = mocks.select.mock.calls[0][1] as string
    progress({
      requestId,
      attachment: { ...attachment().agentAttachment, sizeBytes: 0 },
      receivedBytes: 0,
      status: 'importing'
    })
    await expect
      .element(screen.getByRole('progressbar', { name: 'large.txt' }))
      .toHaveAttribute('aria-valuenow', '0')
    progress({
      requestId,
      attachment: attachment().agentAttachment,
      receivedBytes: 0,
      status: 'importing'
    })
    await expect
      .poll(() => screen.container.querySelector('.composer-attachment')?.getAttribute('title'))
      .toBe('large.txt · 8 B')
    progress({
      requestId,
      attachment: attachment().agentAttachment,
      receivedBytes: 4,
      status: 'importing'
    })
    await expect
      .element(screen.getByRole('progressbar', { name: 'large.txt' }))
      .toHaveAttribute('aria-valuenow', '50')
    pending.resolve([attachment()])
    await expect.element(screen.getByRole('button', { name: 'Send', exact: true })).toBeEnabled()
  })

  it('cancels a still-open native picker request when the composer unmounts', async () => {
    const pending = deferred<ComposerAttachment[]>()
    mocks.select.mockReturnValue(pending.promise)
    const screen = await render(<Harness />)
    await screen.getByRole('button', { name: 'Choose', exact: true }).click()
    const requestId = mocks.select.mock.calls[0][1] as string
    await screen.unmount()
    expect(mocks.cancel).toHaveBeenCalledWith({ importId: requestId })
    pending.resolve([attachment()])
    await Promise.resolve()
    expect(mocks.accepted).not.toHaveBeenCalled()
  })
})
