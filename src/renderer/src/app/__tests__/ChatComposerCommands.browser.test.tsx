import { useState, type ComponentProps } from 'react'
import { userEvent } from 'vitest/browser'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import '../../styles/global.css'

const { execute, fork, submit, changed, translate } = vi.hoisted(() => ({
  execute: vi.fn(),
  fork: vi.fn(),
  submit: vi.fn(),
  changed: vi.fn(),
  translate: (key: string) => key
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: translate })
}))
vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [{ id: 'model-1', displayName: 'Model', supportsImage: true, enabled: true }]
  })
}))
vi.mock('../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({ projects: [], selectProjectDirectory: vi.fn() })
}))
vi.mock('../../features/chat/components/ImagePreview', () => ({ useImagePreview: () => vi.fn() }))
vi.mock('../../features/skills/skillsClient', () => ({ listSkills: vi.fn() }))
vi.mock('../../features/chat/chatAttachments', () => ({
  buildAgentInputAttachments: (attachments: unknown[]) => attachments,
  composerAttachmentFromAgentAttachment: (attachment: unknown) => attachment,
  createComposerAttachmentsFromFiles: async () => [],
  createAttachmentSummary: () => '',
  selectComposerAttachments: async () => [],
  stripAttachmentSummary: (content: string) => content
}))
import { ChatComposer } from '../../features/chat/components/ChatComposer'
import { createComposerDraft } from '../chatMessageFactory'

function TestComposer({
  message = '',
  disabled = false,
  running = false,
  maintenance = false,
  initialDraft
}: {
  message?: string
  disabled?: boolean
  running?: boolean
  maintenance?: boolean
  initialDraft?: ComponentProps<typeof ChatComposer>['draft']
}) {
  const [draft, setDraft] = useState(
    initialDraft ?? createComposerDraft({ modelId: 'model-1', message })
  )
  const update: ComponentProps<typeof ChatComposer>['onDraftChange'] = (next) => {
    changed(next)
    setDraft(next)
  }
  return (
    <div style={{ marginTop: 300, width: 650 }}>
      <ChatComposer
        commands={[
          {
            id: 'compact',
            label: '压缩上下文',
            description: 'Compact',
            disabledReason: disabled ? '聊天空闲时可用' : undefined,
            execute
          },
          { id: 'new', label: '新聊天', description: 'New', execute },
          {
            id: 'fork',
            label: '创建聊天分支',
            description: '从当前最新可用位置创建聊天分支',
            disabledReason: disabled || running || maintenance ? '聊天空闲时可用' : undefined,
            execute: fork
          }
        ]}
        draft={draft}
        onDraftChange={update}
        onDraftMessageChange={update}
        onSubmitMessage={submit}
        isGenerating={running}
        isManualCompactionRunning={maintenance}
      />
    </div>
  )
}
beforeEach(() => {
  vi.clearAllMocks()
})

it('opens only for a typed slash and executes a local command without a message', async () => {
  const view = await render(<TestComposer />)
  const input = view.getByRole('textbox')
  await input.click()
  await userEvent.keyboard('/compact')
  await expect.element(view.getByRole('option', { name: '压缩上下文 Compact' })).toBeVisible()
  await userEvent.keyboard('{Enter}')
  expect(execute).toHaveBeenCalledTimes(1)
  expect(submit).not.toHaveBeenCalled()
  await expect.element(input).toHaveValue('')
  expect(changed.mock.lastCall?.[0]).toMatchObject({
    modelId: 'model-1',
    permissionMode: 'default'
  })
})

it('never queues a disabled command through Enter or form submission during a run', async () => {
  const view = await render(<TestComposer disabled running />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/compact{Enter}')
  document.querySelector('form')!.requestSubmit()
  await userEvent.keyboard('{Escape}{Enter}')
  expect(execute).not.toHaveBeenCalled()
  expect(submit).not.toHaveBeenCalled()
  expect(changed.mock.calls.every(([draft]) => draft.queuedMessages.length === 0)).toBe(true)
  await expect.element(view.getByRole('textbox')).toHaveValue('/compact')
})

it('does not interpret restored slash drafts or pasted paths as commands', async () => {
  const view = await render(<TestComposer message="/compact" />)
  expect(view.container.querySelector('[role="listbox"]')).toBeNull()
  await view.getByRole('textbox').click()
  await userEvent.keyboard('{Enter}')
  expect(submit).toHaveBeenCalledWith('/compact', expect.anything())
  await view.getByRole('textbox').fill('')
  const input = view.container.querySelector('textarea')!
  const clipboardData = new DataTransfer()
  clipboardData.setData('text/plain', '/Users/example/project')
  input.dispatchEvent(new ClipboardEvent('paste', { bubbles: true, clipboardData }))
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(
    input,
    '/Users/example/project'
  )
  input.dispatchEvent(
    new InputEvent('input', {
      bubbles: true,
      inputType: 'insertFromPaste',
      data: '/Users/example/project'
    })
  )
  expect(view.container.querySelector('[role="listbox"]')).toBeNull()
  await userEvent.keyboard('{Enter}')
  expect(submit).toHaveBeenLastCalledWith('/Users/example/project', expect.anything())
  expect(execute).not.toHaveBeenCalled()
})

it('leaves IME confirmation alone, then supports Chinese search and keyboard selection', async () => {
  const view = await render(<TestComposer />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/')
  const input = view.container.querySelector('textarea')!
  input.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }))
  input.dispatchEvent(
    new KeyboardEvent('keydown', { key: 'Enter', isComposing: true, keyCode: 229, bubbles: true })
  )
  input.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }))
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
  expect(execute).not.toHaveBeenCalled()
  expect(submit).not.toHaveBeenCalled()
  await new Promise((resolve) => setTimeout(resolve, 130))
  await userEvent.keyboard('压缩{Enter}')
  expect(execute).toHaveBeenCalledTimes(1)
})

it('allows navigation commands during maintenance and prevents duplicate execution', async () => {
  let resolve!: () => void
  execute.mockImplementationOnce(
    () =>
      new Promise<void>((done) => {
        resolve = done
      })
  )
  const view = await render(<TestComposer disabled maintenance />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/{ArrowDown}{Enter}{Enter}')
  expect(execute).toHaveBeenCalledTimes(1)
  expect(submit).not.toHaveBeenCalled()
  resolve()
})

it('supports mouse execution and sends unknown slash text normally', async () => {
  const view = await render(<TestComposer />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/new')
  await view.getByRole('option').click()
  expect(execute).toHaveBeenCalledTimes(1)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/unknown{Enter}')
  expect(submit).toHaveBeenCalledWith('/unknown', expect.anything())
})

it('uses the hovered row for Enter and highlights matching Chinese text', async () => {
  const view = await render(<TestComposer />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/')
  await userEvent.hover(view.getByRole('option', { name: '新聊天 New' }))
  await expect
    .element(view.getByRole('option', { name: '新聊天 New' }))
    .toHaveAttribute('aria-selected', 'true')
  await userEvent.keyboard('{Enter}')
  expect(execute).toHaveBeenCalledTimes(1)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/压缩')
  expect(view.container.querySelector('.composer-commands__label strong')?.textContent).toBe('压缩')
})

it('routes the send button to the command while preserving attached files and permission settings', async () => {
  const initialDraft = createComposerDraft({
    modelId: 'model-1',
    permissionMode: 'custom',
    attachments: [
      {
        id: 'attachment',
        kind: 'file',
        name: 'keep.txt',
        mimeType: 'text/plain',
        sizeBytes: 4,
        encoding: 'base64',
        data: 'a2VlcA=='
      }
    ]
  })
  const view = await render(<TestComposer initialDraft={initialDraft} />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/compact')
  await view.getByRole('button', { name: '压缩上下文', exact: true }).click()
  expect(execute).toHaveBeenCalledTimes(1)
  expect(submit).not.toHaveBeenCalled()
  expect(changed.mock.lastCall?.[0]).toMatchObject({
    message: '',
    attachments: initialDraft.attachments,
    permissionMode: 'custom',
    modelId: 'model-1'
  })
  await expect.element(view.getByText('keep.txt')).toBeVisible()
})

it('filters the fork command in Chinese and executes the hovered row with Enter', async () => {
  const view = await render(<TestComposer />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/分支')
  await expect.element(view.getByRole('option')).toHaveTextContent('创建聊天分支')
  await userEvent.keyboard('{Escape}')
  await view.getByRole('textbox').fill('')
  await userEvent.keyboard('/')
  await userEvent.hover(
    view.getByRole('option', { name: '创建聊天分支 从当前最新可用位置创建聊天分支' })
  )
  await userEvent.keyboard('{Enter}')
  expect(fork).toHaveBeenCalledTimes(1)
  expect(execute).not.toHaveBeenCalled()
  expect(submit).not.toHaveBeenCalled()
})

it.each([{ running: true }, { maintenance: true }])(
  'keeps /fork out of message and queue paths while busy: %j',
  async (busy) => {
    const view = await render(<TestComposer {...busy} />)
    await view.getByRole('textbox').click()
    await userEvent.keyboard('/fork{Enter}')
    document.querySelector('form')!.requestSubmit()
    await userEvent.keyboard('{Escape}{Enter}')
    expect(fork).not.toHaveBeenCalled()
    expect(submit).not.toHaveBeenCalled()
    expect(changed.mock.calls.every(([draft]) => draft.queuedMessages.length === 0)).toBe(true)
    await expect.element(view.getByRole('textbox')).toHaveValue('/fork')
  }
)
