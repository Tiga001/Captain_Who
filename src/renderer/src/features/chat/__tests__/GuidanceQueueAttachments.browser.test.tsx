import { expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { AgentInputAttachment } from '@mycopilot/protocol'
import type { ChatQueuedMessage } from '../chatTypes'
import { GuidanceQueue } from '../components/GuidanceQueue'
import '../components/GuidanceQueue.css'

const imagePreview = 'data:image/png;base64,AAAA'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) => key,
    language: 'en-US'
  })
}))

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

vi.mock('../chatAttachments', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../chatAttachments')>()),
  loadComposerAttachmentPreview: vi.fn(async () => imagePreview)
}))

function file(id: string): AgentInputAttachment {
  return {
    id,
    kind: 'file',
    name: `${id}.md`,
    mimeType: 'text/markdown',
    sizeBytes: 1024,
    encoding: 'managed',
    data: `managed-${id}`
  }
}

function image(): AgentInputAttachment {
  return {
    id: 'reference-image',
    kind: 'image',
    name: 'reference.png',
    mimeType: 'image/png',
    sizeBytes: 2048,
    encoding: 'managed',
    data: 'managed-reference-image'
  }
}

const message: ChatQueuedMessage = {
  id: 'queued-attachments',
  clientMessageId: 'client-queued-attachments',
  content: 'Review these files',
  attachments: [file('one'), file('two'), file('three'), file('four'), file('five'), image()],
  modelId: 'model-1',
  permissionMode: 'default',
  projectId: null,
  skills: [],
  status: 'pending',
  createdAt: 1
}

it('uses the compact attachment cards for queued messages without destructive controls', async () => {
  const screen = await render(
    <GuidanceQueue
      guideEnabled
      messages={[message]}
      onDelete={vi.fn()}
      onEdit={vi.fn()}
      onGuide={vi.fn()}
      onMove={vi.fn()}
    />
  )

  const cards = screen.container.querySelectorAll<HTMLElement>('.attachment-card')
  expect(cards).toHaveLength(6)
  expect(screen.container.querySelectorAll('.attachment-card-column')).toHaveLength(3)
  expect(
    [...screen.container.querySelectorAll<HTMLElement>('.attachment-card-column')].map(
      (column) => column.children.length
    )
  ).toEqual([4, 1, 1])
  expect(screen.container.querySelectorAll('.composer-attachment__remove')).toHaveLength(0)
  expect(screen.container.querySelectorAll('.workspace-file-type-icon')).toHaveLength(5)
  await expect
    .element(screen.getByRole('img', { name: 'reference.png' }))
    .toHaveAttribute('src', imagePreview)
})
