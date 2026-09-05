import { useState, type CSSProperties } from 'react'
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import type { TranslationKey } from '../../../config/languageRegistry'
import type { ChatConversation } from '../chatTypes'
import '../../../styles/global.css'
import '../../agentCollaboration/AgentCenterPanel.css'

const { copyText } = vi.hoisted(() => ({
  copyText: vi.fn(async () => undefined)
}))
vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/languageRegistry')
  const translate = (key: TranslationKey) => getTranslation('zh-CN', key)
  return { useFrontendConfig: () => ({ t: translate, language: 'zh-CN' }) }
})
vi.mock('../../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [{ id: 'model-1', displayName: 'Model One', enabled: true, supportsImage: true }]
  })
}))
vi.mock('../../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({ projects: [], selectProjectDirectory: vi.fn() })
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

function conversation(turns = 4, longText = false): ChatConversation {
  return {
    id: 'density-chat',
    title: 'Density conversation',
    projectId: null,
    modelId: 'model-1',
    createdAt: 1,
    updatedAt: 2,
    messagesLoaded: true,
    archivedAt: null,
    unreadAt: null,
    messages: Array.from({ length: turns }, (_, index) => [
      {
        id: `user-${index}`,
        role: 'user' as const,
        content: `用户请求 ${index}`,
        createdAt: 1000 + index,
        status: 'sent' as const
      },
      {
        id: `assistant-${index}`,
        role: 'assistant' as const,
        content: longText
          ? `${'这是一段用于验证窄聊天区换行和滚动的长中文正文，包含 English 和文件路径。'.repeat(8)}\n\n- 第一项\n  - 嵌套说明\n- 第二项`
          : `助手回答 ${index}`,
        createdAt: 2000 + index,
        status: 'sent' as const
      }
    ]).flat()
  }
}

function Workspace({
  middleWidth = 720,
  shellHeight = 760,
  halfHeight = false,
  turns = 4,
  longText = false,
  observer = false
}: {
  middleWidth?: number
  shellHeight?: number
  halfHeight?: boolean
  turns?: number
  longText?: boolean
  observer?: boolean
}) {
  const [bottomOpen, setBottomOpen] = useState(halfHeight)
  const [draft, setDraft] = useState(() => createComposerDraft({ modelId: 'model-1' }))
  const [chat] = useState(() => conversation(turns, longText))
  const common = { conversation: chat, initialScrollTop: 0, showTokenUsageDetails: false }
  return (
    <div
      className="app-shell"
      data-left-open="true"
      data-right-open="true"
      data-bottom-open={String(bottomOpen)}
      style={
        {
          width: middleWidth + 360,
          height: shellHeight,
          '--left-panel-width': '160px',
          '--right-panel-width': '200px',
          '--bottom-panel-height': bottomOpen ? `${shellHeight / 2}px` : '0px'
        } as CSSProperties
      }
    >
      <aside className="side-panel side-panel--left">
        <button type="button" onClick={() => setBottomOpen((value) => !value)}>
          toggle bottom
        </button>
      </aside>
      <main className="main-panel">
        <div className="main-panel__toolbar">Conversation</div>
        <div className={`main-panel__surface${observer ? ' agent-center__observer' : ''}`}>
          {observer ? (
            <ConversationSurface {...common} mode="observer" rootConversationId="root-chat" />
          ) : (
            <ConversationSurface
              {...common}
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
          )}
        </div>
      </main>
      <aside className="side-panel side-panel--right">Review</aside>
      <aside className="side-panel side-panel--bottom">Terminal</aside>
    </div>
  )
}

function element(selector: string): HTMLElement {
  const found = document.querySelector<HTMLElement>(selector)
  if (!found) throw new Error(`Missing ${selector}`)
  return found
}
const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
async function scrollToEnd() {
  // Let ConversationSurface restore the initial scroll position before scrolling explicitly.
  await frame()
  const scroller = element('.chat-conversation-page__messages')
  scroller.scrollTop = scroller.scrollHeight
  await frame()
  return scroller
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

describe('Conversation density and adaptive message layout', () => {
  it.each([
    { width: 480, padding: 24, rail: false },
    { width: 720, padding: 32, rail: true },
    { width: 900, padding: 40, rail: true }
  ])(
    'uses the same $width px middle-column layout at different application widths',
    async ({ width, padding, rail }) => {
      const screen = await render(<Workspace middleWidth={width} />)
      const scroller = element('.chat-conversation-page__messages')
      for (const viewportWidth of [1600, width + 360]) {
        await page.viewport(viewportWidth, 1000)
        expect(getComputedStyle(scroller).paddingLeft).toBe(`${padding}px`)
        expect(getComputedStyle(scroller).paddingRight).toBe(`${padding}px`)
        const navigation = element('.conversation-turn-navigation')
        expect(getComputedStyle(navigation).display === 'none').toBe(!rail)
        if (rail) {
          const row = element('.conversation-turn-navigation__row').getBoundingClientRect()
          const text = element('.chat-message--assistant').getBoundingClientRect()
          expect(row.right + 2).toBeLessThanOrEqual(text.left)
        }
      }
      await screen.unmount()
    }
  )

  it('keeps the hover path and copy hit area clear of the next message', async () => {
    await render(<Workspace middleWidth={720} />)
    const message = element('[data-message-id="assistant-0"]')
    const next = element('[data-message-id="user-1"]')
    const action = message.querySelector<HTMLButtonElement>('[aria-label="复制消息"]')!
    await userEvent.hover(message)
    const box = message.getBoundingClientRect()
    const actionBox = action.getBoundingClientRect()
    expect(actionBox.width).toBe(24)
    expect(actionBox.height).toBe(24)
    expect(next.getBoundingClientRect().top - box.bottom).toBe(40)
    expect(actionBox.bottom + 3).toBeLessThan(next.getBoundingClientRect().top)
    // Move through the actual pseudo-element bridge, then click without a layout-stability wait.
    await page.elementLocator(message).hover({ position: { x: 8, y: box.height + 3 }, force: true })
    expect(document.elementFromPoint(box.left + 8, box.bottom + 3)).toBe(message)
    await page.elementLocator(action).click({ force: true })
    expect(copyText).toHaveBeenCalledExactlyOnceWith('助手回答 0')
    const nextBody = next.querySelector<HTMLElement>('.chat-message__body')!.getBoundingClientRect()
    expect(
      document.elementFromPoint(nextBody.left + 8, nextBody.top + 8)?.closest('[data-message-id]')
    ).toBe(next)
  })

  it('keeps the final actions reachable above the fixed composer with a half-height terminal', async () => {
    await render(<Workspace middleWidth={480} halfHeight longText />)
    const composer = element('.chat-conversation-page__composer')
    const composerBox = composer.getBoundingClientRect().toJSON()
    const terminalBox = element('.side-panel--bottom').getBoundingClientRect().toJSON()
    const scroller = await scrollToEnd()
    const action = element('[data-message-id="assistant-3"] [aria-label="复制消息"]')
    action.focus({ preventScroll: true })
    expect(action.getBoundingClientRect().bottom + 3).toBeLessThanOrEqual(
      scroller.getBoundingClientRect().bottom
    )
    expect(scroller.scrollWidth).toBeLessThanOrEqual(scroller.clientWidth + 1)
    await userEvent.keyboard('{Enter}')
    expect(copyText).toHaveBeenCalledOnce()
    expect(composer.getBoundingClientRect().toJSON()).toEqual(composerBox)
    expect(element('.side-panel--bottom').getBoundingClientRect().toJSON()).toEqual(terminalBox)
    expect(composer.getBoundingClientRect().bottom).toBeLessThanOrEqual(terminalBox.top)
    await page.getByRole('button', { name: 'toggle bottom' }).click()
    await expect.poll(() => composer.getBoundingClientRect().bottom).toBe(760)
    await scrollToEnd()
    expect(action.getBoundingClientRect().bottom + 3).toBeLessThanOrEqual(
      scroller.getBoundingClientRect().bottom
    )
  })

  it('bounds a long turn navigator to the shortened message viewport and keeps its last turn reachable', async () => {
    await render(<Workspace middleWidth={720} halfHeight turns={60} />)
    const region = element('.chat-conversation-page__messages-region')
    const list = element('.conversation-turn-navigation__list')
    await expect.poll(() => list.scrollHeight > list.clientHeight).toBe(true)
    expect(list.getBoundingClientRect().top).toBeGreaterThanOrEqual(
      region.getBoundingClientRect().top
    )
    expect(list.getBoundingClientRect().bottom).toBeLessThanOrEqual(
      region.getBoundingClientRect().bottom
    )
    list.scrollTop = list.scrollHeight
    await frame()
    const last = list.lastElementChild as HTMLButtonElement
    expect(last.getBoundingClientRect().bottom).toBeLessThanOrEqual(
      list.getBoundingClientRect().bottom
    )
    await page.elementLocator(last).click()
    await expect
      .poll(() => element('.chat-conversation-page__messages').scrollTop)
      .toBeGreaterThan(1000)
  })

  it('applies the narrow layout to a read-only observer without adding a composer', async () => {
    await render(<Workspace middleWidth={320} observer longText />)
    const scroller = await scrollToEnd()
    expect(getComputedStyle(scroller).paddingLeft).toBe('14px')
    expect(scroller.scrollWidth).toBeLessThanOrEqual(scroller.clientWidth + 1)
    expect(getComputedStyle(element('.conversation-turn-navigation')).display).toBe('none')
    expect(document.querySelector('.chat-composer')).toBeNull()
    const action = element('[data-message-id="assistant-3"] [aria-label="复制消息"]')
    expect(action.getBoundingClientRect().bottom + 3).toBeLessThanOrEqual(
      scroller.getBoundingClientRect().bottom
    )
  })
})
