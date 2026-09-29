import { useState } from 'react'
import type { AgentInputAttachment, AttachmentImportMetadata } from '@mycopilot/protocol'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { createComposerDraft } from '../chatMessageFactory'
import { ChatComposer } from '../../features/chat/components/ChatComposer'
import type { ChatComposerDraft } from '../../features/chat/chatTypes'
import '../../styles/global.css'

const mocks = vi.hoisted(() => ({
  begin: vi.fn(),
  append: vi.fn(),
  finish: vi.fn(),
  cancel: vi.fn(),
  loadText: vi.fn(),
  submit: vi.fn(),
  changed: vi.fn(),
  translate: (key: string) => (key === 'chat.pastedTextFileName' ? '粘贴的文本.txt' : key)
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    attachments: {
      beginImport: mocks.begin,
      appendImport: mocks.append,
      finishImport: mocks.finish,
      cancelImport: mocks.cancel,
      loadText: mocks.loadText,
      loadPreview: async () => undefined
    }
  }
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: mocks.translate
  })
}))
vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [
      {
        id: 'model',
        displayName: 'Model',
        providerModelId: 'model',
        supportsImage: true,
        enabled: true
      }
    ]
  })
}))
vi.mock('../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({ projects: [], openCreateProjectDialog: vi.fn() })
}))
vi.mock('../../features/chat/components/ImagePreview', () => ({ useImagePreview: () => vi.fn() }))
vi.mock('../../features/skills/skillsClient', () => ({
  listSkills: async () => ({
    schemaVersion: 1,
    catalogRevision: 'test',
    skills: [],
    diagnostics: [],
    truncated: false
  })
}))

function managed(id = 'pasted'): AgentInputAttachment {
  return {
    id,
    name: '粘贴的文本.txt',
    kind: 'file',
    mimeType: 'text/plain',
    sizeBytes: 5000,
    encoding: 'managed',
    data: id,
    pastedText: { preview: 'Pasted preview', characterCount: 5000 }
  }
}

function Harness({
  initial = createComposerDraft({ modelId: 'model', message: 'before after' }),
  generating = false
}: {
  initial?: ChatComposerDraft
  generating?: boolean
}) {
  const [scope, setScope] = useState('first')
  const [drafts, setDrafts] = useState<Record<string, ChatComposerDraft>>({
    first: initial,
    second: createComposerDraft({ modelId: 'model', message: 'second draft' })
  })
  const update = (draft: ChatComposerDraft) => {
    mocks.changed(scope, draft)
    setDrafts((values) => ({ ...values, [scope]: draft }))
  }
  return (
    <div style={{ width: 700, margin: 50 }}>
      <button onClick={() => setScope((current) => (current === 'first' ? 'second' : 'first'))}>
        Switch draft
      </button>
      <ChatComposer
        draft={drafts[scope]}
        resetKey={scope}
        onDraftChange={update}
        onDraftMessageChange={update}
        onSubmitMessage={mocks.submit}
        isGenerating={generating}
      />
      <output data-testid="drafts">{JSON.stringify(drafts)}</output>
    </div>
  )
}

function paste(textarea: HTMLTextAreaElement, text: string, file?: File) {
  const clipboardData = new DataTransfer()
  clipboardData.setData('text/plain', text)
  if (file) clipboardData.items.add(file)
  const event = new ClipboardEvent('paste', { clipboardData, bubbles: true, cancelable: true })
  textarea.dispatchEvent(event)
  return event
}

function caret(textarea: HTMLTextAreaElement, start: number, end = start) {
  textarea.focus()
  textarea.setSelectionRange(start, end)
  textarea.dispatchEvent(new Event('select', { bubbles: true }))
}

beforeEach(() => {
  vi.clearAllMocks()
  const metadata = new Map<string, AttachmentImportMetadata>()
  mocks.begin.mockImplementation(async (input: AttachmentImportMetadata) => {
    metadata.set(input.id, input)
    return { importId: input.id }
  })
  mocks.append.mockImplementation(async ({ offset, data }) => ({
    receivedBytes: offset + atob(data).length
  }))
  mocks.finish.mockImplementation(async ({ importId }) => ({
    ...metadata.get(importId),
    encoding: 'managed',
    data: importId
  }))
  mocks.cancel.mockResolvedValue(undefined)
  mocks.submit.mockResolvedValue(false)
})

it('converts a single paste at 5000 UTF-16 units, preserves existing input, and imports exact UTF-8 text', async () => {
  const screen = await render(<Harness />)
  const textarea = screen.getByRole('textbox').element() as HTMLTextAreaElement
  expect(paste(textarea, 'x'.repeat(4999)).defaultPrevented).toBe(false)
  expect(mocks.begin).not.toHaveBeenCalled()
  const text = '中文\r\n🙂 '.repeat(1000)
  expect(paste(textarea, text).defaultPrevented).toBe(true)
  await expect.poll(() => mocks.finish.mock.calls.length).toBe(1)
  expect(textarea.value).toBe('before after')
  expect(mocks.begin.mock.calls[0][0]).toMatchObject({
    name: '粘贴的文本.txt',
    kind: 'file',
    mimeType: 'text/plain',
    sizeBytes: new TextEncoder().encode(text).length,
    pastedText: {
      preview: Array.from(text.replace(/\s+/gu, ' ').trim()).slice(0, 80).join(''),
      characterCount: text.length
    }
  })
  const bytes = Uint8Array.from(atob(mocks.append.mock.calls[0][0].data), (value) =>
    value.charCodeAt(0)
  )
  expect(new TextDecoder().decode(bytes)).toBe(text)
  expect(screen.container.querySelector('.composer-attachment__file-button')).toBeNull()
  await screen.getByRole('button', { name: 'chat.send', exact: true }).click()
  expect(mocks.submit.mock.calls[0][0]).toBe('before after')
  expect(mocks.submit.mock.calls[0][1].attachments[0].pastedText.characterCount).toBe(text.length)
})

it('gives clipboard files priority over long plain text', async () => {
  const screen = await render(<Harness />)
  paste(
    screen.getByRole('textbox').element() as HTMLTextAreaElement,
    'x'.repeat(5000),
    new File(['file'], 'ordinary.txt', { type: 'text/plain' })
  )
  await expect.poll(() => mocks.finish.mock.calls.length).toBe(1)
  expect(mocks.begin.mock.calls[0][0]).toMatchObject({ name: 'ordinary.txt' })
  expect(mocks.begin.mock.calls[0][0].pastedText).toBeUndefined()
})

it('restores full managed text at the remembered selection without reconverting it', async () => {
  const text = '🙂\n'.repeat(10_000)
  mocks.loadText.mockResolvedValue({ text })
  const screen = await render(
    <Harness
      initial={createComposerDraft({
        modelId: 'model',
        message: 'before REPLACE after',
        attachments: [managed()]
      })}
    />
  )
  const textarea = screen.getByRole('textbox').element() as HTMLTextAreaElement
  caret(textarea, 7, 14)
  await screen.getByRole('button', { name: 'chat.showPastedTextInEditor' }).click()
  await expect.poll(() => textarea.value).toBe(`before ${text} after`)
  await expect.poll(() => textarea.selectionStart).toBe(7 + text.length)
  expect(screen.container.querySelector('.composer-attachment')).toBeNull()
  expect(mocks.begin).not.toHaveBeenCalled()
  expect(mocks.loadText).toHaveBeenCalledExactlyOnceWith({ attachment: managed() })
})

it('keeps failed imports recoverable and disables submission until restored', async () => {
  mocks.begin.mockRejectedValue(new Error('disk full'))
  const screen = await render(<Harness />)
  const textarea = screen.getByRole('textbox').element() as HTMLTextAreaElement
  caret(textarea, 7)
  const text = 'x'.repeat(5000)
  paste(textarea, text)
  await expect
    .element(screen.getByRole('button', { name: 'files.retry 粘贴的文本.txt' }))
    .toBeVisible()
  await expect
    .element(screen.getByRole('button', { name: 'chat.send', exact: true }))
    .toBeDisabled()
  await screen.getByRole('button', { name: 'chat.showPastedTextInEditor' }).click()
  await expect.poll(() => textarea.value).toBe(`before ${text}after`)
  expect(mocks.loadText).not.toHaveBeenCalled()
  await expect.element(screen.getByRole('button', { name: 'chat.send', exact: true })).toBeEnabled()
})

it('keeps the attachment and current input if managed text loading fails', async () => {
  mocks.loadText.mockRejectedValue(new Error('missing file'))
  const screen = await render(
    <Harness
      initial={createComposerDraft({
        modelId: 'model',
        message: 'keep me',
        attachments: [managed()]
      })}
    />
  )
  await screen.getByRole('button', { name: 'chat.showPastedTextInEditor' }).click()
  await expect
    .element(screen.getByRole('button', { name: 'chat.showPastedTextInEditor' }))
    .toBeEnabled()
  expect((screen.getByRole('textbox').element() as HTMLTextAreaElement).value).toBe('keep me')
  expect(screen.container.querySelector('.composer-attachment')).not.toBeNull()
})

it('recovers a pending paste to its source draft on a scope switch and ignores the late importer', async () => {
  let finish!: (input: { importId: string }) => void
  mocks.begin.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve
      })
  )
  const screen = await render(<Harness />)
  const text = 'pending'.repeat(1000)
  paste(screen.getByRole('textbox').element() as HTMLTextAreaElement, text)
  await expect
    .element(screen.getByRole('button', { name: 'chat.send', exact: true }))
    .toBeDisabled()
  await screen.getByRole('textbox').fill('latest source edits')
  await screen.getByRole('button', { name: 'Switch draft' }).click()
  expect((screen.getByRole('textbox').element() as HTMLTextAreaElement).value).toBe('second draft')
  const recovered = mocks.changed.mock.calls.find(
    ([scope, value]) => scope === 'first' && value.message.includes(text)
  )
  expect(recovered?.[1].message).toBe(`latest source edits\n\n${text}`)
  finish({ importId: 'late' })
  await expect.poll(() => mocks.cancel.mock.calls.length).toBe(1)
  await screen.getByRole('button', { name: 'Switch draft' }).click()
  expect((screen.getByRole('textbox').element() as HTMLTextAreaElement).value).toBe(
    `latest source edits\n\n${text}`
  )
  expect(screen.container.querySelector('.composer-attachment')).toBeNull()
})

it('does not insert a late restore into a different draft or resurrect a removed attachment', async () => {
  let resolve!: (value: { text: string }) => void
  mocks.loadText.mockImplementation(
    () =>
      new Promise((done) => {
        resolve = done
      })
  )
  const screen = await render(
    <Harness
      initial={createComposerDraft({ modelId: 'model', message: 'keep', attachments: [managed()] })}
    />
  )
  await screen.getByRole('button', { name: 'chat.showPastedTextInEditor' }).click()
  await screen.getByRole('button', { name: 'chat.removeAttachment 粘贴的文本.txt' }).click()
  resolve({ text: 'late'.repeat(2000) })
  await expect.element(screen.getByRole('button', { name: 'chat.send', exact: true })).toBeEnabled()
  expect((screen.getByRole('textbox').element() as HTMLTextAreaElement).value).toBe('keep')
  await screen.unmount()
  const switched = await render(
    <Harness
      initial={createComposerDraft({ modelId: 'model', message: 'keep', attachments: [managed()] })}
    />
  )
  await switched.getByRole('button', { name: 'chat.showPastedTextInEditor' }).click()
  await switched.getByRole('button', { name: 'Switch draft' }).click()
  resolve({ text: 'late'.repeat(2000) })
  await expect
    .poll(() => (switched.getByRole('textbox').element() as HTMLTextAreaElement).value)
    .toBe('second draft')
})

it('preserves the pasted metadata when queueing a message without exposing a restore action on the queue', async () => {
  const screen = await render(
    <Harness
      generating
      initial={createComposerDraft({
        modelId: 'model',
        message: 'guide',
        attachments: [managed()]
      })}
    />
  )
  await screen.getByRole('button', { name: 'chat.queueMessage', exact: true }).click()
  const draft = mocks.changed.mock.calls.at(-1)?.[1]
  expect(draft.queuedMessages[0].attachments).toEqual([managed()])
  expect(draft.attachments).toEqual([])
  expect(
    screen.container.querySelector('.attachment-card-list--queue .composer-attachment__restore')
  ).toBeNull()
  expect(screen.container.querySelector('.attachment-card-list--queue')?.textContent).toContain(
    'Pasted preview'
  )
})

it('preserves input typed while managed text is loading', async () => {
  let resolve!: (value: { text: string }) => void
  mocks.loadText.mockImplementation(
    () =>
      new Promise((done) => {
        resolve = done
      })
  )
  const screen = await render(
    <Harness
      initial={createComposerDraft({
        modelId: 'model',
        message: 'before after',
        attachments: [managed()]
      })}
    />
  )
  const textarea = screen.getByRole('textbox').element() as HTMLTextAreaElement
  caret(textarea, 7)
  await screen.getByRole('button', { name: 'chat.showPastedTextInEditor' }).click()
  await screen.getByRole('textbox').fill('before after newly typed')
  const text = 'restored'.repeat(1000)
  resolve({ text })
  await expect.poll(() => textarea.value).toBe(`before ${text}after newly typed`)
  expect(screen.container.querySelector('.composer-attachment')).toBeNull()
})
