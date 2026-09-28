import { createElement, useState, type ComponentProps } from 'react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { cdp, page, server, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type ReactMarkdown from 'react-markdown'
import type { HumanInteractionHostApi } from '@mycopilot/host-api'
import type { ChatConversation, ChatMessage } from '../chatTypes'
import type {
  ConversationScrollAnchor,
  ConversationScrollPosition
} from '../useConversationSegments'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import '../../../styles/global.css'
import {
  fakeHost,
  question,
  submitted
} from '../../humanInteraction/__tests__/humanInteractionFixtures'

const probes = vi.hoisted(() => ({
  markdown: 0,
  copied: '',
  copyToBrowser: false,
  submit: vi.fn(),
  noop: () => undefined,
  api: undefined as HumanInteractionHostApi | undefined
}))
vi.mock('react-markdown', async (original) => {
  const module = await original<typeof import('react-markdown')>()
  return {
    ...module,
    default: (props: ComponentProps<typeof ReactMarkdown>) => {
      probes.markdown += 1
      return createElement(module.default, props)
    }
  }
})
vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/languageRegistry')
  return {
    useFrontendConfig: () => ({
      language: 'zh-CN',
      t: (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
    })
  }
})
vi.mock('../../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [{ id: 'model', displayName: 'Model', enabled: true, supportsImage: true }]
  })
}))
vi.mock('../../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({ projects: [], openCreateProjectDialog: probes.noop })
}))
vi.mock('../../skills/useSkillCatalog', () => ({
  useSkillCatalog: () => ({ state: { status: 'idle' }, refresh: probes.noop })
}))
vi.mock('../../../host/hostClient', () => ({
  hostClient: {
    get humanInteraction() {
      return probes.api
    }
  }
}))
vi.mock('../../../components/clipboard', () => ({
  copyTextToClipboard: vi.fn(async (value: string) => {
    probes.copied = value
    if (probes.copyToBrowser) await navigator.clipboard.writeText(value)
  })
}))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => probes.noop,
  useImagePreviewNotice: () => probes.noop
}))
const [{ ConversationSurface }, { createComposerDraft }] = await Promise.all([
  import('../ConversationSurface'),
  import('../../../app/chatMessageFactory')
])

const thumbnail =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII='

function conversation(turns: number, id = 'segments-chat'): ChatConversation {
  const messages: ChatMessage[] = []
  for (let index = 0; index < turns; index += 1) {
    const callId = `call-${index}`
    messages.push({
      id: `user-${index}`,
      role: 'user',
      content: `问题 ${index}：解释 **性能优化**，保留 unique-search-${index}。`,
      status: 'sent',
      createdAt: index * 2 + 1,
      ...(index % 4 === 0
        ? {
            attachments: [
              {
                id: `image-${index}`,
                name: `diagram-${index}.png`,
                kind: 'image' as const,
                mimeType: 'image/png',
                sizeBytes: 68,
                previewData: thumbnail,
                previewMimeType: 'image/png'
              }
            ]
          }
        : {})
    })
    messages.push({
      id: `assistant-${index}`,
      role: 'assistant',
      content: `## 回答 ${index}\n\n${'保留可靠的历史定位与交互状态。'.repeat(12)}\n\n- 分段挂载\n- 可访问的历史\n\n\`\`\`typescript\nconst answer = ${index};\nconsole.log(answer);\n\`\`\`\n\n结束 ${index}。`,
      status: 'sent',
      createdAt: index * 2 + 2,
      ...(index % 3 === 0
        ? {
            agentRun: {
              runId: `run-${index}`,
              status: 'completed' as const,
              startedAt: index * 2 + 1,
              completedAt: index * 2 + 2,
              toolDefinitions: [],
              toolCalls: [
                {
                  id: callId,
                  tool: 'read_file',
                  args: { path: `file-${index}.ts` },
                  approvalStatus: 'not_required' as const,
                  reason: null
                }
              ],
              toolResults: [
                {
                  callId,
                  tool: 'read_file',
                  ok: true,
                  result: { content: `const value = ${index};` }
                }
              ],
              approvals: [],
              fileChangeProposals: [],
              timeline: []
            }
          }
        : {})
    })
  }
  return {
    id,
    title: '分段历史',
    projectId: null,
    modelId: 'model',
    createdAt: 1,
    updatedAt: 2,
    archivedAt: null,
    unreadAt: null,
    messagesLoaded: true,
    messages
  }
}

function Workspace({
  chat,
  target,
  width = 1100,
  initialScrollTop = null,
  onScroll
}: {
  chat: ChatConversation
  target?: string
  width?: number
  initialScrollTop?: ConversationScrollPosition | null
  onScroll?: (id: string, top: number, anchor?: ConversationScrollAnchor) => void
}) {
  const [draft, setDraft] = useState(() => createComposerDraft({ modelId: 'model' }))
  return (
    <div style={{ width, height: 780 }}>
      <ConversationSurface
        conversation={chat}
        scrollTargetMessageId={target}
        initialScrollTop={initialScrollTop}
        onScrollPositionChange={onScroll}
        mode="interactive"
        showTokenUsageDetails={false}
        composerDraft={draft}
        onComposerDraftChange={setDraft}
        editSelectedModelAvailable
        editSelectedModelSupportsImage
        onSubmitMessage={probes.submit}
        onMessageUiStateChange={probes.noop}
        permissionModeAvailability={{ custom: true, full: true }}
      />
    </div>
  )
}

function scroller() {
  return document.querySelector<HTMLDivElement>('.chat-conversation-page__messages')!
}
const frame = () => new Promise<number>((resolve) => requestAnimationFrame(resolve))
async function frames(count = 2) {
  for (let i = 0; i < count; i += 1) await frame()
}

let oldStyle: string | null
beforeEach(async () => {
  oldStyle = document.documentElement.getAttribute('style')
  for (const [key, value] of Object.entries(getFrontendCssVariables())) {
    document.documentElement.style.setProperty(key, value)
  }
  probes.markdown = 0
  probes.copied = ''
  probes.copyToBrowser = false
  probes.api = undefined
  await page.viewport(1200, 900)
})
afterEach(() => {
  if (oldStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', oldStyle)
})

function message(id: string) {
  return document.querySelector<HTMLElement>(`[data-message-id="${id}"]`)
}
function top(id: string) {
  const element = message(id)
  return element
    ? element.getBoundingClientRect().top - scroller().getBoundingClientRect().top
    : Infinity
}
async function jump(index: number) {
  const button = document.querySelector<HTMLButtonElement>(`[data-turn-id="user-${index}"]`)!
  button.click()
  await expect.poll(() => Math.abs(top(`user-${index}`))).toBeLessThan(2)
}

function enterHistoryQuery(query: string) {
  const input = document.querySelector<HTMLInputElement>(
    '.conversation-history-tools__search input'
  )!
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, query)
  input.dispatchEvent(new Event('input', { bubbles: true }))
}

it('bounds initial Markdown mounts and reaches an unmounted turn by click and rail drag', async () => {
  await render(<Workspace chat={conversation(1000)} />)
  await frames(4)
  expect(document.querySelectorAll('[data-message-id]').length).toBeLessThanOrEqual(48)
  expect(probes.markdown).toBeLessThanOrEqual(48)
  expect(message('user-100')).toBeNull()
  await jump(100)
  expect(message('assistant-100')).not.toBeNull()
  const list = document.querySelector<HTMLElement>('.conversation-turn-navigation__list')!
  const first = document
    .querySelector<HTMLElement>('[data-turn-id="user-100"]')!
    .getBoundingClientRect()
  const target = document
    .querySelector<HTMLElement>('[data-turn-id="user-500"]')!
    .getBoundingClientRect()
  for (const [type, y] of [
    ['pointerdown', first.top],
    ['pointermove', target.top + target.height / 2],
    ['pointerup', target.top]
  ] as const) {
    list.dispatchEvent(
      new PointerEvent(type, {
        bubbles: true,
        button: 0,
        buttons: type === 'pointerup' ? 0 : 1,
        clientY: y,
        isPrimary: true,
        pointerId: 61,
        pointerType: 'mouse'
      })
    )
  }
  await expect.poll(() => message('user-500')).not.toBeNull()
})

it('mounts a history reference target and restores a message anchor after remount', async () => {
  const chat = conversation(300)
  let remembered: ConversationScrollAnchor | undefined
  const remember = (_id: string, _top: number, value?: ConversationScrollAnchor) => {
    if (value) remembered = value
  }
  const first = await render(<Workspace chat={chat} onScroll={remember} />)
  await jump(100)
  scroller().scrollTop += 24
  await frames(4)
  expect(remembered?.messageId).toBeTruthy()
  const saved = { ...remembered! }
  await first.unmount()
  await render(<Workspace chat={chat} initialScrollTop={saved} />)
  await expect.poll(() => Math.abs(top(saved.messageId) - saved.offset)).toBeLessThan(2)
})

it('mounts explicit unread or reference targets even when the same conversation is already open', async () => {
  const chat = conversation(300)
  const view = await render(<Workspace chat={chat} target="user-40" />)
  await expect.poll(() => top('user-40')).toBeGreaterThanOrEqual(0)
  await expect.poll(() => top('user-40')).toBeLessThan(scroller().clientHeight)
  expect(message('user-170')).toBeNull()
  await view.rerender(<Workspace chat={chat} target="user-170" />)
  await expect.poll(() => top('user-170')).toBeLessThan(scroller().clientHeight)
  expect(top('user-170')).toBeGreaterThanOrEqual(0)
})

it('keeps a history anchor through asynchronous height changes and streaming updates', async () => {
  const chat = conversation(300)
  const view = await render(<Workspace chat={chat} />)
  await jump(103)
  await frames(3)
  const before = top('user-103')
  const earlier = message('assistant-102')!
  const image = document.createElement('div')
  image.style.height = '240px'
  earlier.append(image)
  await frames(6)
  expect(Math.abs(top('user-103') - before)).toBeLessThan(2)
  const tail = chat.messages.at(-1)!
  await view.rerender(
    <Workspace
      chat={{
        ...chat,
        messages: [
          ...chat.messages.slice(0, -1),
          { ...tail, status: 'pending', content: tail.content + '\n\n' + 'stream '.repeat(300) }
        ]
      }}
    />
  )
  await frames(6)
  expect(Math.abs(top('user-103') - before)).toBeLessThan(2)
  expect(scroller().scrollHeight - scroller().scrollTop - scroller().clientHeight).toBeGreaterThan(
    1000
  )
})

it('loads the complete history for find and copies a selection across unloaded segments without truncation', async () => {
  await cdp().send('Page.bringToFront')
  const { targetInfo } = await cdp().send('Target.getTargetInfo')
  await cdp().send('Browser.grantPermissions', {
    permissions: ['clipboardReadWrite', 'clipboardSanitizedWrite'],
    origin: location.origin,
    browserContextId: targetInfo.browserContextId
  })
  await cdp().send('Emulation.setFocusEmulationEnabled', { enabled: true })
  probes.copyToBrowser = true
  await render(<Workspace chat={conversation(100)} />)
  await jump(0)
  window.focus()
  expect(message('user-50')).toBeNull()
  const range = document.createRange()
  range.setStart(message('user-0')!, 0)
  range.setEnd(message('assistant-99')!, message('assistant-99')!.childNodes.length)
  const selection = window.getSelection()!
  selection.removeAllRanges()
  selection.addRange(range)
  const copy = new ClipboardEvent('copy', {
    bubbles: true,
    cancelable: true,
    clipboardData: new DataTransfer()
  })
  scroller().dispatchEvent(copy)
  expect(copy.defaultPrevented).toBe(true)
  await expect.poll(() => probes.copied).toContain('unique-search-50')
  expect(probes.copied).toContain('unique-search-0')
  expect(probes.copied).toContain('结束 99')
  expect(await navigator.clipboard.readText()).toBe(probes.copied)
  await page.getByRole('button', { name: '查找对话', exact: true }).click()
  await page.getByRole('textbox', { name: '搜索对话内容' }).fill('unique-search-50')
  await expect.poll(() => Math.abs(top('user-50'))).toBeLessThan(scroller().clientHeight)
  await expect.poll(() => window.getSelection()?.toString()).toBe('unique-search-50')
  const searchField = page.getByRole('textbox', { name: '搜索对话内容' }).element()
  const focusOut = vi.fn()
  searchField.addEventListener('focusout', focusOut)
  await userEvent.keyboard('{Escape}')
  expect(focusOut).toHaveBeenCalledOnce()
  expect(document.querySelector('.conversation-history-tools__search')).toBeNull()
})

it('cancels full-history expansion when changing conversations', async () => {
  const view = await render(<Workspace chat={conversation(1000)} />)
  await page.getByRole('button', { name: '展开全部历史' }).click()
  await view.rerender(<Workspace chat={conversation(1000, 'another-chat')} />)
  await frames(8)
  expect(document.querySelectorAll('[data-message-id]').length).toBeLessThanOrEqual(48)
  expect(document.querySelector('[role="status"]')?.textContent ?? '').not.toContain('正在展开')
})

it('keeps existing message DOM and local state when a short conversation crosses the threshold', async () => {
  const chat = conversation(32)
  const view = await render(<Workspace chat={chat} />)
  await jump(0)
  const first = message('user-0')!
  first.dataset.localState = 'retained'
  const extra = { ...chat.messages[0], id: 'new-user' }
  await view.rerender(<Workspace chat={{ ...chat, messages: [...chat.messages, extra] }} />)
  await frames(4)
  expect(message('user-0')).toBe(first)
  expect(message('user-0')?.dataset.localState).toBe('retained')
  expect(Math.abs(top('user-0'))).toBeLessThan(2)
})

it('pins historical running and approval turns and retains them after completion', async () => {
  const chat = conversation(300)
  const pending = chat.messages.map((item) =>
    item.id === 'assistant-3' || item.id === 'assistant-60'
      ? {
          ...item,
          status: 'pending' as const,
          agentRun: {
            ...item.agentRun!,
            status:
              item.id === 'assistant-3' ? ('running' as const) : ('waiting_for_approval' as const)
          }
        }
      : item
  )
  const view = await render(<Workspace chat={{ ...chat, messages: pending }} />)
  await frames(3)
  const first = message('assistant-3')
  const second = message('assistant-60')
  expect(first).not.toBeNull()
  expect(second).not.toBeNull()
  await view.rerender(<Workspace chat={chat} />)
  await frames(3)
  expect(message('assistant-3')).toBe(first)
  expect(message('assistant-60')).toBe(second)
})

it('keeps independent anchors when the same surface switches A to B to A', async () => {
  const a = conversation(300, 'a')
  const b = {
    ...conversation(300, 'b'),
    messages: conversation(300, 'b').messages.map((item) => ({ ...item, id: `b-${item.id}` }))
  }
  const positions = new Map<string, ConversationScrollAnchor>()
  const remember = (id: string, _top: number, value?: ConversationScrollAnchor) => {
    if (value) positions.set(id, value)
  }
  const view = await render(<Workspace chat={a} onScroll={remember} />)
  await jump(100)
  scroller().scrollTop += 36
  await frames(4)
  const saved = { ...positions.get('a')! }
  await view.rerender(<Workspace chat={b} onScroll={remember} />)
  await frames(4)
  expect(positions.get('a')).toEqual(saved)
  await view.rerender(
    <Workspace chat={a} onScroll={remember} initialScrollTop={positions.get('a')} />
  )
  await expect.poll(() => Math.abs(top(saved.messageId) - saved.offset)).toBeLessThan(2)
})

it('mounts the section reached by a direct scrollbar jump without snapping back to the tail', async () => {
  await render(<Workspace chat={conversation(300)} />)
  await frames(3)
  const gap = document.querySelector<HTMLElement>('[data-message-segment="20"]')!
  expect(gap.querySelector('[data-message-id]')).toBeNull()
  scroller().scrollTop +=
    gap.getBoundingClientRect().top - scroller().getBoundingClientRect().top + gap.clientHeight / 2
  await expect.poll(() => gap.querySelector('[data-message-id]')).not.toBeNull()
  await frames(6)
  expect(gap.getBoundingClientRect().top).toBeLessThan(scroller().getBoundingClientRect().bottom)
  expect(gap.getBoundingClientRect().bottom).toBeGreaterThan(scroller().getBoundingClientRect().top)
  expect(scroller().scrollHeight - scroller().scrollTop - scroller().clientHeight).toBeGreaterThan(
    1000
  )
})

it('preserves a reading anchor when the chat width changes', async () => {
  const chat = conversation(300)
  const view = await render(<Workspace chat={chat} />)
  await jump(103)
  await frames(3)
  const before = top('user-103')
  await view.rerender(<Workspace chat={chat} width={660} />)
  await frames(6)
  expect(Math.abs(top('user-103') - before)).toBeLessThan(2)
})

it('preserves an immediate select-all then copy and finds unmounted history from a focused composer', async () => {
  await render(<Workspace chat={conversation(300)} />)
  window.getSelection()?.removeAllRanges()
  ;(document.activeElement as HTMLElement)?.blur()
  document.body.dispatchEvent(
    new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'a', metaKey: true })
  )
  const copy = new ClipboardEvent('copy', {
    bubbles: true,
    cancelable: true,
    clipboardData: new DataTransfer()
  })
  document.body.dispatchEvent(copy)
  expect(copy.defaultPrevented).toBe(true)
  await expect.poll(() => probes.copied, { timeout: 5000 }).toContain('unique-search-0')
  expect(probes.copied).toContain('unique-search-150')
  expect(probes.copied).toContain('结束 299')
  const composer =
    document.querySelector<HTMLElement>('[contenteditable="true"]') ??
    document.querySelector<HTMLElement>('textarea')!
  composer.focus()
  await userEvent.keyboard('{Meta>}f{/Meta}')
  await expect.element(page.getByRole('textbox', { name: '搜索对话内容' })).toBeVisible()
  await page.getByRole('textbox', { name: '搜索对话内容' }).fill('unique-search-40')
  await expect.poll(() => window.getSelection()?.toString()).toBe('unique-search-40')
})

it('intercepts find from the focused composer before historical messages have mounted', async () => {
  await render(<Workspace chat={conversation(100)} />)
  expect(message('user-40')).toBeNull()
  const composer =
    document.querySelector<HTMLElement>('[contenteditable="true"]') ??
    document.querySelector<HTMLElement>('textarea')!
  composer.focus()
  await userEvent.keyboard('{Meta>}f{/Meta}')
  await page.getByRole('textbox', { name: '搜索对话内容' }).fill('unique-search-40')
  await expect.poll(() => window.getSelection()?.toString()).toBe('unique-search-40')
  expect(message('assistant-99')).not.toBeNull()
})

it('pins an open historical interaction and keeps the projected answer and message boundaries intact', async () => {
  const chat = conversation(100)
  const request = {
    ...question('historical-question', 1, chat.id),
    assistantMessageId: 'assistant-7',
    runId: 'run-question'
  }
  const source = chat.messages[7 * 2 + 1]
  const active = {
    ...source,
    agentRun: {
      ...chat.messages[7].agentRun!,
      runId: request.runId,
      toolCalls: [
        {
          id: request.toolCallId,
          tool: 'request_user_input_async',
          args: { questions: request.questions },
          approvalStatus: 'not_required' as const,
          reason: null
        }
      ],
      toolResults: [
        {
          callId: request.toolCallId,
          tool: 'request_user_input_async',
          ok: true,
          result: { accepted: true, requestId: request.requestId }
        }
      ],
      timeline: [{ id: 'question-call', type: 'tool_call' as const, callId: request.toolCallId }]
    }
  }
  chat.messages = chat.messages.map((item) => (item.id === source.id ? active : item))
  const host = fakeHost([request])
  probes.api = host.api
  await render(<Workspace chat={chat} />)
  await expect.poll(() => message('assistant-7')).not.toBeNull()
  const pinned = message('assistant-7')!
  const input = page.getByRole('textbox', { name: '自定义回答' })
  await input.fill('保留输入中的答案')
  await jump(80)
  await expect.element(input).toHaveValue('保留输入中的答案')
  expect(host.api.submit).not.toHaveBeenCalled()
  const answered = submitted(request, {
    requestId: request.requestId,
    conversationId: chat.id,
    expectedRevision: 0,
    submissionId: 'answer-once',
    answers: [
      { questionId: request.questions[0].id, kind: 'text', text: '历史答案' },
      { questionId: request.questions[1].id, kind: 'skipped' }
    ]
  })
  host.notify(answered)
  await frames(4)
  expect(message('assistant-7')).toBe(pinned)
  await page.getByRole('button', { name: '展开全部历史' }).click()
  await expect.poll(() => document.querySelectorAll('[data-segment-placeholder]').length).toBe(0)
  expect(document.querySelectorAll('[data-message-id]').length).toBe(200)
  expect(message('assistant-99')).not.toBeNull()
  const interactionSummary = [...pinned.querySelectorAll<HTMLButtonElement>('button')].find(
    (button) => button.textContent?.includes('交互')
  )
  interactionSummary?.click()
  await frames(2)
  expect(pinned.textContent).toContain('历史答案')
})

it('does not intercept global find or select-all while the retained workspace is inert or hidden', async () => {
  await render(<Workspace chat={conversation(100)} />)
  const surface = document.querySelector<HTMLElement>('.conversation-surface')!
  for (const attribute of ['inert', 'hidden', 'aria-hidden']) {
    surface.setAttribute(attribute, 'true')
    for (const key of ['f', 'a']) {
      const shortcut = new KeyboardEvent('keydown', {
        key,
        metaKey: true,
        bubbles: true,
        cancelable: true
      })
      document.body.dispatchEvent(shortcut)
      expect(shortcut.defaultPrevented).toBe(false)
    }
    surface.removeAttribute(attribute)
  }
  expect(document.querySelector('.conversation-history-tools__search')).toBeNull()
})

it('keeps resize observation bounded and releases every observed target on unmount', async () => {
  const nativeObserve = ResizeObserver.prototype.observe
  const nativeDisconnect = ResizeObserver.prototype.disconnect
  const targets = new Map<ResizeObserver, Set<Element>>()
  const observe = vi.spyOn(ResizeObserver.prototype, 'observe').mockImplementation(function (
    this: ResizeObserver,
    target,
    options
  ) {
    const items = targets.get(this) ?? new Set<Element>()
    items.add(target)
    targets.set(this, items)
    nativeObserve.call(this, target, options)
  })
  const disconnect = vi.spyOn(ResizeObserver.prototype, 'disconnect').mockImplementation(function (
    this: ResizeObserver
  ) {
    targets.delete(this)
    nativeDisconnect.call(this)
  })
  try {
    const counts: number[] = []
    for (const turns of [300, 1000]) {
      const view = await render(<Workspace chat={conversation(turns)} />)
      await frames(3)
      counts.push([...targets.values()].reduce((count, items) => count + items.size, 0))
      await view.unmount()
      expect(targets.size).toBe(0)
    }
    expect(counts[1]).toBe(counts[0])
    expect(counts[1]).toBeLessThanOrEqual(8)
  } finally {
    observe.mockRestore()
    disconnect.mockRestore()
  }
})

it.runIf(Boolean(import.meta.env.VITE_CONVERSATION_RENDER_BENCH))(
  'records actual Chromium mixed-history mount, layout, heap and scroll costs',
  async () => {
    const samples: unknown[] = []
    const protocol = cdp()
    await protocol.send('Performance.enable')
    for (const turns of [300, 1000]) {
      const chat = conversation(turns, `bench-${turns}`)
      await protocol.send('HeapProfiler.collectGarbage')
      const beforeHeap = await protocol.send('Runtime.getHeapUsage')
      const beforeMetrics = await protocol.send('Performance.getMetrics')
      const before = new Map(beforeMetrics.metrics.map((entry) => [entry.name, entry.value]))
      probes.markdown = 0
      const start = performance.now()
      const view = await render(<Workspace chat={chat} />)
      await frames()
      const mountMs = performance.now() - start
      const initialMarkdownParses = probes.markdown
      const initialMessages = document.querySelectorAll('[data-message-id]').length
      const initialDom = document.querySelectorAll('*').length
      const mountedMetrics = await protocol.send('Performance.getMetrics')
      const mounted = new Map(mountedMetrics.metrics.map((entry) => [entry.name, entry.value]))
      await protocol.send('HeapProfiler.collectGarbage')
      const afterHeap = await protocol.send('Runtime.getHeapUsage')
      const intervals: number[] = []
      let previous = await frame()
      for (let index = 0; index < 60; index += 1) {
        scroller().scrollTop = Math.max(0, scroller().scrollTop - 85)
        const now = await frame()
        intervals.push(now - previous)
        previous = now
      }
      intervals.sort((a, b) => a - b)
      const sample = {
        turns,
        mountMs,
        initialMarkdownParses,
        initialMessages,
        initialDom,
        usedHeapBefore: beforeHeap.usedSize,
        usedHeapAfter: afterHeap.usedSize,
        retainedHeapDelta: afterHeap.usedSize - beforeHeap.usedSize,
        layoutMs:
          ((mounted.get('LayoutDuration') ?? 0) - (before.get('LayoutDuration') ?? 0)) * 1000,
        scriptMs:
          ((mounted.get('ScriptDuration') ?? 0) - (before.get('ScriptDuration') ?? 0)) * 1000,
        scrollFrameP95Ms: intervals[Math.floor(intervals.length * 0.95)]
      }
      if (turns === 1000 && import.meta.env.VITE_CONVERSATION_RENDER_BENCH === 'release-search') {
        document.querySelector<HTMLButtonElement>('[aria-label="查找对话"]')!.click()
        await frames(2)
        enterHistoryQuery('unique-search-40')
        const deadline = performance.now() + 20000
        while (
          window.getSelection()?.toString() !== 'unique-search-40' &&
          performance.now() < deadline
        )
          await frame()
        expect(window.getSelection()?.toString()).toBe('unique-search-40')
        document
          .querySelector<HTMLInputElement>('.conversation-history-tools__search input')!
          .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
        await frames(2)
        expect(document.querySelector('.conversation-history-tools__search')).toBeNull()
      } else if (
        turns === 1000 &&
        import.meta.env.VITE_CONVERSATION_RENDER_BENCH === 'release-pure'
      ) {
        document
          .querySelector<HTMLButtonElement>('.conversation-history-tools__actions button')!
          .click()
        await expect
          .poll(() => document.querySelectorAll('[data-segment-placeholder]').length, {
            timeout: 15000
          })
          .toBe(0)
      } else if (
        turns === 1000 &&
        !String(import.meta.env.VITE_CONVERSATION_RENDER_BENCH).startsWith('baseline')
      ) {
        scroller().scrollTop = scroller().scrollHeight
        await frames(3)
        await page.screenshot({
          path: '../../../../../../.cache/performance-audit/round3-render/default-1000.png'
        })
        await page.getByRole('button', { name: '查找对话', exact: true }).click()
        await page.getByRole('textbox', { name: '搜索对话内容' }).fill('unique-search-40')
        await expect
          .poll(() => window.getSelection()?.toString(), { timeout: 15000 })
          .toBe('unique-search-40')
        await page.screenshot({
          path: '../../../../../../.cache/performance-audit/round3-render/find-history.png'
        })
      }
      await view.unmount()
      await frames()
      await protocol.send('HeapProfiler.collectGarbage')
      const releasedHeap = await protocol.send('Runtime.getHeapUsage')
      window.getSelection()?.removeAllRanges()
      await frames(12)
      await protocol.send('HeapProfiler.collectGarbage')
      const idleHeap = await protocol.send('Runtime.getHeapUsage')
      const domCounters = await protocol.send('Memory.getDOMCounters')
      samples.push({
        ...sample,
        usedHeapAfterUnmount: releasedHeap.usedSize,
        usedHeapAfterIdle: idleHeap.usedSize,
        domCountersAfterUnmount: domCounters,
        messagesAfterUnmount: document.querySelectorAll('[data-message-id]').length,
        domAfterUnmount: document.querySelectorAll('*').length
      })
    }
    const label = String(import.meta.env.VITE_CONVERSATION_RENDER_BENCH)
    await server.commands.writeFile(
      `.cache/performance-audit/round3-render-${label}.json`,
      JSON.stringify(samples, null, 2)
    )
    console.log('CONVERSATION_RENDER_BENCH', JSON.stringify(samples))
    expect(samples).toHaveLength(2)
  },
  120_000
)
