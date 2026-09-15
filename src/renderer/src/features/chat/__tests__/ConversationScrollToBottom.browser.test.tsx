import { useState, type CSSProperties } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import { getTranslation, type TranslationKey } from '../../../config/languageRegistry'
import type { ChatConversation, ChatMessage } from '../chatTypes'
import '../../../styles/global.css'
import '../../agentCollaboration/AgentCenterPanel.css'

const { copyText } = vi.hoisted(() => ({
  copyText: vi.fn(async () => undefined)
}))

vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation: translate } = await import('../../../config/languageRegistry')
  const t = (key: TranslationKey) => translate('zh-CN', key)
  return { useFrontendConfig: () => ({ t, language: 'zh-CN' }) }
})
vi.mock('../../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [{ id: 'model-1', displayName: 'Model One', enabled: true, supportsImage: true }]
  })
}))
vi.mock('../../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({ projects: [], openCreateProjectDialog: vi.fn(async () => null) })
}))
vi.mock('../../skills/useSkillCatalog', () => ({
  useSkillCatalog: () => ({ state: { status: 'idle' }, refresh: vi.fn() })
}))
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../components/clipboard', () => ({ copyTextToClipboard: copyText }))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))

const [{ ConversationSurface }, { createComposerDraft }] = await Promise.all([
  import('../ConversationSurface'),
  import('../../../app/chatMessageFactory')
])

const scrollToBottomLabel = getTranslation('zh-CN', 'chat.scrollToBottom')
const bodyRepeat = '这是用于撑高滚动区域的正文内容。'

function scrollConversation(
  options: { turns?: number; pending?: boolean; tail?: string } = {}
): ChatConversation {
  const { turns = 6, pending = false, tail = '' } = options
  const messages: ChatMessage[] = []
  for (let index = 0; index < turns; index += 1) {
    const isLast = index === turns - 1
    messages.push({
      id: `user-${index}`,
      role: 'user',
      content: `第 ${index} 个问题：${'展开说说滚动跟随的边界情况。'.repeat(4)}`,
      createdAt: 1000 + index * 10,
      status: 'sent'
    })
    messages.push({
      id: `assistant-${index}`,
      role: 'assistant',
      content: `第 ${index} 个回答：${isLast ? tail : ''}${bodyRepeat.repeat(12)}`,
      createdAt: 2000 + index * 10,
      status: isLast && pending ? 'pending' : 'sent'
    })
  }
  return {
    id: 'scroll-follow-chat',
    title: '滚动跟随',
    projectId: null,
    modelId: 'model-1',
    createdAt: 1,
    updatedAt: 2,
    messagesLoaded: true,
    archivedAt: null,
    unreadAt: null,
    messages
  }
}

function Workspace({
  chat,
  initialScrollTop = null
}: {
  chat: ChatConversation
  initialScrollTop?: number | null
}) {
  const [draft, setDraft] = useState(() => createComposerDraft({ modelId: 'model-1' }))
  return (
    <div
      className="app-shell"
      data-left-open="true"
      data-right-open="true"
      data-bottom-open="false"
      style={
        {
          width: 1560,
          height: 760,
          '--left-panel-width': '160px',
          '--right-panel-width': '200px',
          '--bottom-panel-height': '0px'
        } as CSSProperties
      }
    >
      <aside className="side-panel side-panel--left">Sessions</aside>
      <main className="main-panel">
        <div className="main-panel__toolbar">Conversation</div>
        <div className="main-panel__surface">
          <ConversationSurface
            conversation={chat}
            initialScrollTop={initialScrollTop}
            showTokenUsageDetails={false}
            mode="interactive"
            composerDraft={draft}
            onComposerDraftChange={setDraft}
            editSelectedModelAvailable
            editSelectedModelSupportsImage
            onSubmitMessage={vi.fn()}
            onMessageUiStateChange={vi.fn()}
            onContinueInNewTask={vi.fn()}
            permissionModeAvailability={{ custom: true, full: true }}
          />
        </div>
      </main>
      <aside className="side-panel side-panel--right">Review</aside>
    </div>
  )
}

function element(selector: string): HTMLElement {
  const found = document.querySelector<HTMLElement>(selector)
  if (!found) throw new Error(`Missing ${selector}`)
  return found
}
function control(): HTMLElement | null {
  return document.querySelector<HTMLElement>('.conversation-scroll-to-bottom')
}
function requiredControl(): HTMLElement {
  const found = control()
  if (!found) throw new Error('Missing .conversation-scroll-to-bottom')
  return found
}
function scroller(): HTMLElement {
  return element('.chat-conversation-page__messages')
}
function distanceFromBottom(node: HTMLElement): number {
  return Math.max(0, node.scrollHeight - node.scrollTop - node.clientHeight)
}
const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
async function frames(count: number) {
  for (let index = 0; index < count; index += 1) await frame()
}

let previousRootStyle: string | null
beforeEach(async () => {
  copyText.mockClear()
  previousRootStyle = document.documentElement.getAttribute('style')
  for (const [name, value] of Object.entries(getFrontendCssVariables()))
    document.documentElement.style.setProperty(name, value)
  await page.viewport(1600, 1000)
})
afterEach(async () => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
  await page.viewport(1280, 720)
})

describe('conversation scroll-to-bottom control', () => {
  it('stays hidden at the bottom, appears above the composer after scrolling up, and returns on click', async () => {
    await render(<Workspace chat={scrollConversation()} />)
    const node = scroller()

    await expect.poll(() => distanceFromBottom(node)).toBeLessThanOrEqual(1)
    expect(control()).toBeNull()

    // Scrolling up while idle reveals the down-arrow control.
    node.scrollTop = Math.max(0, node.scrollTop - 700)
    await expect.poll(() => control()).not.toBeNull()
    const button = requiredControl()
    expect(button.getAttribute('aria-label')).toBe(scrollToBottomLabel)
    expect(button.getAttribute('data-generating')).toBeNull()
    expect(button.querySelector('svg')).not.toBeNull()

    // It floats above the composer, right-aligned with the composer card, and does not travel
    // with the message list.
    const buttonBox = button.getBoundingClientRect()
    const cardBox = element('.chat-composer').getBoundingClientRect()
    expect(Math.abs(buttonBox.right - cardBox.right)).toBeLessThanOrEqual(1)
    expect(buttonBox.bottom).toBeLessThan(cardBox.top)
    node.scrollTop = Math.max(0, node.scrollTop - 400)
    await frames(2)
    const movedButtonBox = requiredControl().getBoundingClientRect()
    expect(movedButtonBox.left).toBe(buttonBox.left)
    expect(movedButtonBox.top).toBe(buttonBox.top)

    // Clicking returns to the newest content and hides the control again.
    await page.elementLocator(button).click()
    await expect.poll(() => distanceFromBottom(scroller())).toBeLessThanOrEqual(1)
    await expect.poll(() => control()).toBeNull()
  })

  it('shows the typing ellipsis while a reply streams and follows new content until the reader scrolls away', async () => {
    const screen = await render(<Workspace chat={scrollConversation({ pending: true })} />)
    const node = scroller()

    // At the bottom while generating: still hidden.
    await expect.poll(() => distanceFromBottom(node)).toBeLessThanOrEqual(1)
    expect(control()).toBeNull()

    node.scrollTop = Math.max(0, node.scrollTop - 700)
    await expect.poll(() => control()).not.toBeNull()
    const typingButton = requiredControl()
    expect(typingButton.getAttribute('data-generating')).toBe('true')
    expect(typingButton.querySelector('svg')).toBeNull()
    expect(
      typingButton.querySelectorAll('.conversation-scroll-to-bottom__typing > span').length
    ).toBe(3)

    // Clicking returns to the bottom and the viewport keeps following the streaming reply.
    await page.elementLocator(typingButton).click()
    await expect.poll(() => distanceFromBottom(scroller())).toBeLessThanOrEqual(1)
    await expect.poll(() => control()).toBeNull()
    await screen.rerender(
      <Workspace chat={scrollConversation({ pending: true, tail: '流式增长内容。'.repeat(100) })} />
    )
    await expect.poll(() => distanceFromBottom(scroller())).toBeLessThanOrEqual(1)

    // A reader scrolling away stops the follow; new content must not yank the view back.
    node.scrollTop = Math.max(0, node.scrollTop - 700)
    await expect.poll(() => control()).not.toBeNull()
    const away = distanceFromBottom(node)
    expect(away).toBeGreaterThan(100)
    const scrollTopBefore = node.scrollTop
    await screen.rerender(
      <Workspace chat={scrollConversation({ pending: true, tail: '继续增长段落。'.repeat(200) })} />
    )
    await frames(2)
    expect(node.scrollTop).toBeLessThanOrEqual(scrollTopBefore + 2)
    await expect.poll(() => distanceFromBottom(node)).toBeGreaterThan(away)
    expect(control()).not.toBeNull()

    // Once the reply settles while the view is still away, the control falls back to the arrow.
    await expect.poll(() => control()?.getAttribute('data-generating')).toBe('true')
    await screen.rerender(
      <Workspace
        chat={scrollConversation({ pending: false, tail: '继续增长段落。'.repeat(200) })}
      />
    )
    await expect.poll(() => control()?.getAttribute('data-generating')).toBeNull()
    expect(requiredControl().querySelector('svg')).not.toBeNull()

    // The control still brings the reader back to the bottom.
    await page.elementLocator(requiredControl()).click()
    await expect.poll(() => distanceFromBottom(scroller())).toBeLessThanOrEqual(1)
    await expect.poll(() => control()).toBeNull()
  })

  it('keeps a restored reading position and its control without following later content', async () => {
    const screen = await render(
      <Workspace chat={scrollConversation({ pending: true })} initialScrollTop={250} />
    )
    const node = scroller()

    await expect.poll(() => Math.abs(node.scrollTop - 250) <= 1).toBe(true)
    await expect.poll(() => control()).not.toBeNull()
    expect(requiredControl().getAttribute('data-generating')).toBe('true')
    expect(distanceFromBottom(node)).toBeGreaterThan(20)

    await screen.rerender(
      <Workspace
        chat={scrollConversation({ pending: true, tail: '后台继续输出内容。'.repeat(120) })}
        initialScrollTop={250}
      />
    )
    await frames(2)
    expect(Math.abs(node.scrollTop - 250)).toBeLessThanOrEqual(2)
    expect(control()).not.toBeNull()
  })
})
