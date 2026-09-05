import type { AgentUsage } from '@mycopilot/protocol'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import type { ChatMessage } from '../chatTypes'
import '../../../styles/global.css'
import '../ChatConversationPage.css'
import '../../agentCollaboration/AgentCenterPanel.css'

const settings = vi.hoisted(() => ({ language: 'zh-CN' as 'zh-CN' | 'en-US' }))
vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/languageRegistry')
  return {
    useFrontendConfig: () => ({
      language: settings.language,
      t: (key: Parameters<typeof getTranslation>[1]) => getTranslation(settings.language, key)
    })
  }
})
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: vi.fn() })
}))
vi.mock('../../../components/clipboard', () => ({
  copyTextToClipboard: vi.fn(async () => undefined)
}))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))

const { ChatMessageItem } = await import('../components/ChatMessageItem')

const usage: AgentUsage = {
  inputTokens: 987_654_321,
  outputTokens: 98_765_432,
  outputThinkingTokens: 87_654_321,
  totalTokens: 1_086_419_753,
  cachedInputTokens: 765_432_109,
  cacheCreationInputTokens: 123_456_789,
  billableRequestCount: 43
}

function messages(): ChatMessage[] {
  return [
    {
      id: 'overflow-user',
      role: 'user',
      content:
        '请检查消息操作与工具状态。\nPlease inspect message actions and tool status.\n保留完整反馈。',
      createdAt: Date.now(),
      status: 'sent'
    },
    {
      id: 'overflow-assistant',
      role: 'assistant',
      content: '检查已完成。\n\nThe inspection is complete.\n\n包含中文和 English 说明。',
      createdAt: Date.now(),
      status: 'sent',
      agentRun: {
        runId: 'overflow-run',
        status: 'completed',
        startedAt: Date.now() - 30_000,
        completedAt: Date.now(),
        toolDefinitions: [],
        toolCalls: [],
        toolResults: [],
        approvals: [],
        fileChangeProposals: [],
        timeline: [],
        usage
      }
    }
  ]
}

function Fixture({ width, observer }: { width: number; observer: boolean }) {
  return (
    <div
      className={observer ? 'agent-center__observer' : undefined}
      style={{ width, height: 650, margin: '40px 100px' }}
    >
      <section className="chat-conversation-page conversation-surface">
        <div className="chat-conversation-page__messages-region">
          <div className="chat-conversation-page__messages">
            {messages().map((message) => (
              <ChatMessageItem
                key={message.id}
                message={message}
                mode={observer ? 'observer' : 'interactive'}
                onContinueInNewTask={observer ? undefined : vi.fn()}
                onUiStateChange={observer ? undefined : vi.fn()}
                showTokenUsageDetails
              />
            ))}
          </div>
        </div>
      </section>
    </div>
  )
}

function required<T extends HTMLElement = HTMLElement>(root: ParentNode, selector: string): T {
  const result = root.querySelector<T>(selector)
  if (!result) throw new Error(`Missing ${selector}`)
  return result
}

function expectNoHorizontalOverflow(scroller: HTMLElement, context: string) {
  expect
    .soft(scroller.scrollWidth, `${context}: message scrollWidth`)
    .toBeLessThanOrEqual(scroller.clientWidth + 1)
}

function expectTooltipVisible(tooltip: HTMLElement, scroller: HTMLElement, context: string) {
  const tooltipBox = tooltip.getBoundingClientRect()
  const viewport = scroller.getBoundingClientRect()
  expect.soft(getComputedStyle(tooltip).opacity, `${context}: tooltip opacity`).toBe('1')
  expect.soft(tooltipBox.width, `${context}: tooltip width`).toBeGreaterThan(0)
  expect.soft(tooltipBox.left, `${context}: left clipping`).toBeGreaterThanOrEqual(viewport.left)
  expect.soft(tooltipBox.right, `${context}: right clipping`).toBeLessThanOrEqual(viewport.right)
  expect.soft(tooltipBox.top, `${context}: top clipping`).toBeGreaterThanOrEqual(viewport.top)
  expect.soft(tooltipBox.bottom, `${context}: bottom clipping`).toBeLessThanOrEqual(viewport.bottom)
  expect
    .soft(tooltip.scrollWidth, `${context}: tooltip text overflow`)
    .toBeLessThanOrEqual(tooltip.clientWidth + 1)
}

let previousRootStyle: string | null
beforeEach(async () => {
  previousRootStyle = document.documentElement.getAttribute('style')
  for (const [name, value] of Object.entries(getFrontendCssVariables())) {
    document.documentElement.style.setProperty(name, value)
  }
  await page.viewport(1600, 900)
})
afterEach(async () => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
  await page.viewport(1280, 720)
})

describe('Message action overflow in narrow translated layouts', () => {
  it.each(
    (['zh-CN', 'en-US'] as const).flatMap((language) =>
      [
        { observer: true, width: 280 },
        { observer: false, width: 320 }
      ].flatMap((layout) =>
        (['hover', 'focus'] as const).map((interaction) => ({ language, ...layout, interaction }))
      )
    )
  )(
    '$language $width px observer=$observer $interaction',
    async ({ language, width, observer, interaction }) => {
      settings.language = language
      const screen = await render(<Fixture width={width} observer={observer} />)
      await document.fonts.ready
      const scroller = required(screen.container, '.chat-conversation-page__messages')
      expectNoHorizontalOverflow(scroller, 'all actions hidden')

      const buttons = Array.from(
        scroller.querySelectorAll<HTMLButtonElement>('.chat-message__actions button')
      )
      expect(buttons).toHaveLength(observer ? 3 : 5)
      for (const button of buttons) {
        const context = `${interaction}: ${button.getAttribute('aria-label')}`
        if (interaction === 'hover') {
          await userEvent.hover(button.closest('.chat-message') as HTMLElement)
          await userEvent.hover(button)
        } else {
          button.focus({ preventScroll: true })
        }
        const tooltip = button.closest('.chat-message__usage')
          ? required(button.parentElement!, '.chat-message__usage-popover')
          : required(button, '.chat-message__action-tooltip')
        await expect.poll(() => getComputedStyle(tooltip).opacity).toBe('1')
        expectNoHorizontalOverflow(scroller, context)
        expectTooltipVisible(tooltip, scroller, context)
        button.blur()
        await userEvent.hover(document.body)
      }
    }
  )
})
