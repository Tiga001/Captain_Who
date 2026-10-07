import { useRef, type CSSProperties } from 'react'
import { expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { getTranslation, type TranslationKey } from '../../../config/frontendTranslations'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import { crabLightTheme } from '../../../config/themes/crab'
import { ConversationTurnNavigationRail } from '../components/ConversationTurnNavigationRail'
import {
  getConversationTurnNavigationItems,
  type ConversationTurnNavigationItem
} from '../conversationTurnNavigation'
import type { ChatMessage } from '../chatTypes'
import '../../../styles/global.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) =>
      key.startsWith('chat.turnNavigationSource')
        ? getTranslation('zh-CN', key as TranslationKey)
        : key
  })
}))
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

const items: ConversationTurnNavigationItem[] = Array.from({ length: 4 }, (_, index) => ({
  favorited: index === 1,
  source: index === 2 ? 'agent' : 'human',
  ...(index === 2 ? { sourceLabel: '公式复核', senderAgentId: 'review-agent' } : {}),
  id: `user-${index + 1}`,
  userMessageId: `user-${index + 1}`,
  userPreview: `User ${index + 1}`,
  assistantPreview: `Assistant ${index + 1}`
}))

function RailHarness() {
  const scrollContainerRef = useRef<HTMLDivElement>(null)

  return (
    <div>
      <div className="scroll-root" ref={scrollContainerRef}>
        {items.map((item) => (
          <div key={item.id}>
            <article
              className="chat-message chat-message--user"
              data-message-id={item.userMessageId}
            >
              <div className="chat-message__body">{item.userPreview}</div>
            </article>
            <article
              className="chat-message chat-message--assistant"
              data-message-id={`assistant-${item.id}`}
            >
              {item.assistantPreview}
            </article>
          </div>
        ))}
      </div>
      <ConversationTurnNavigationRail items={items} scrollContainerRef={scrollContainerRef} />
    </div>
  )
}

const denseItems: ConversationTurnNavigationItem[] = Array.from({ length: 36 }, (_, index) => {
  const source = [0, 3, 18, 30].includes(index)
    ? 'human'
    : index === 12
      ? 'workflow'
      : index === 28
        ? 'context'
        : 'agent'
  return {
    favorited: index === 3 || index === 25,
    source,
    ...(source === 'agent' ? { senderAgentId: 'review-agent' } : {}),
    ...(source === 'workflow' ? { sourceLabel: '每日复核' } : {}),
    id: `dense-${index + 1}`,
    userMessageId: `dense-${index + 1}`,
    userPreview: `第 ${index + 1} 轮：复核报告中的关键结论`,
    assistantPreview: '已经复核相关证据，并将结果整理到项目文件中。'
  }
})

function DenseRailHarness({ onRevealMessage }: { onRevealMessage: (id: string) => void }) {
  const scrollContainerRef = useRef<HTMLDivElement>(null)
  return (
    <div
      data-testid="dense-rail"
      style={
        {
          position: 'relative',
          width: 780,
          height: 560,
          padding: '48px 64px',
          boxSizing: 'border-box',
          background: 'var(--mc-color-surface-main-panel)',
          color: 'var(--mc-color-text-primary)',
          fontFamily: 'var(--mc-font-family)',
          fontSize: 14,
          lineHeight: 1.7
        } as CSSProperties
      }
    >
      <button data-testid="away">回到对话</button>
      <div ref={scrollContainerRef} style={{ marginTop: 100 }}>
        <p>请检查这份报告的关键结论，确认引用和计算过程一致。</p>
        <p>已经完成复核，相关证据和结果保存在项目文件中。左侧刻度记录这段对话的位置。</p>
      </div>
      <ConversationTurnNavigationRail
        agentLabelsById={{ 'review-agent': '公式复核' }}
        items={denseItems}
        messageTurnIds={denseItems.map((item) => ({ id: item.id, role: 'user' }))}
        onRevealMessage={onRevealMessage}
        scrollContainerRef={scrollContainerRef}
        visibleMessageIds={new Set(['dense-18', 'dense-19'])}
      />
    </div>
  )
}

function ScrollableRailHarness({ count, height }: { count: number; height: number }) {
  const scrollContainerRef = useRef<HTMLDivElement>(null)
  const scrollItems = Array.from({ length: count }, (_, index) => ({
    ...denseItems[index % denseItems.length],
    id: `scroll-${index + 1}`,
    userMessageId: `scroll-${index + 1}`,
    userPreview: `第 ${index + 1} 项：复核关键结论`
  }))
  return (
    <div
      data-testid="scrollable-rail"
      style={
        {
          position: 'relative',
          width: 780,
          height,
          padding: '24px 64px',
          background: 'var(--mc-color-surface-main-panel)',
          color: 'var(--mc-color-text-primary)',
          fontFamily: 'var(--mc-font-family)',
          fontSize: 14,
          lineHeight: 1.7
        } as CSSProperties
      }
    >
      <button data-testid="scroll-away">回到对话</button>
      <div ref={scrollContainerRef}>
        <p>刻度指向历史消息；只有仍可继续滚动的一端呈现渐隐。</p>
      </div>
      <ConversationTurnNavigationRail
        items={scrollItems}
        messageTurnIds={scrollItems.map((item) => ({ id: item.id, role: 'user' }))}
        onRevealMessage={() => {}}
        scrollContainerRef={scrollContainerRef}
        visibleMessageIds={new Set()}
      />
    </div>
  )
}

const consecutiveMessages: ChatMessage[] = [
  { id: 'earlier', role: 'user', content: '先检查引用', createdAt: 1, status: 'sent' },
  { id: 'earlier-answer', role: 'assistant', content: '引用已检查', createdAt: 2, status: 'sent' },
  { id: 'human-one', role: 'user', content: '继续核对计算', createdAt: 3, status: 'sent' },
  {
    id: 'agent-mail-one',
    role: 'user',
    content: '**核对完成**：参考 [报告](https://example.com/report) 得到结果。',
    createdAt: 4,
    status: 'sent',
    inputOrigin: {
      kind: 'agent',
      senderAgentId: 'review-agent',
      sourceAgentMessageId: 'source-review-message',
      snapshotSourceConversationId: null,
      snapshotSourceMessageId: null
    }
  },
  { id: 'human-two', role: 'user', content: '也检查单位', createdAt: 5, status: 'sent' },
  {
    id: 'shared-answer',
    role: 'assistant',
    content: '计算与单位均已核对',
    createdAt: 6,
    status: 'sent'
  },
  { id: 'pending-user', role: 'user', content: '再看下一份报告', createdAt: 7, status: 'sent' },
  { id: 'pending-answer', role: 'assistant', content: '正在查看', createdAt: 8, status: 'pending' }
]

function ConsecutiveInputHarness({
  visibleMessageIds,
  onRevealMessage
}: {
  visibleMessageIds: string[]
  onRevealMessage: (id: string) => void
}) {
  const scrollContainerRef = useRef<HTMLDivElement>(null)
  return (
    <div>
      <div ref={scrollContainerRef}>
        {consecutiveMessages.map((message) => (
          <article
            className={`chat-message chat-message--${message.role}`}
            data-message-id={message.id}
            key={message.id}
          >
            {message.content}
          </article>
        ))}
      </div>
      <ConversationTurnNavigationRail
        agentLabelsById={{ 'review-agent': '公式复核' }}
        items={getConversationTurnNavigationItems(consecutiveMessages)}
        messageTurnIds={consecutiveMessages}
        onRevealMessage={onRevealMessage}
        scrollContainerRef={scrollContainerRef}
        visibleMessageIds={new Set(visibleMessageIds)}
      />
    </div>
  )
}

function createRect(top: number, height: number): DOMRect {
  return {
    x: 0,
    y: top,
    top,
    right: 400,
    bottom: top + height,
    left: 0,
    width: 400,
    height,
    toJSON: () => ({})
  }
}

function getRequiredElement(selector: string) {
  const element = document.querySelector<HTMLElement>(selector)
  if (!element) {
    throw new Error(`Missing test element: ${selector}`)
  }
  return element
}

function createIntersectionEntry(
  target: Element,
  isIntersecting: boolean,
  intersectionRatio: number
): IntersectionObserverEntry {
  const targetRect = target.getBoundingClientRect()
  return {
    boundingClientRect: targetRect,
    intersectionRatio,
    intersectionRect: isIntersecting ? targetRect : createRect(0, 0),
    isIntersecting,
    rootBounds: null,
    target,
    time: 0
  }
}

function dispatchPointerEvent(
  target: Element,
  type: 'pointerdown' | 'pointermove' | 'pointerup',
  clientY: number
) {
  target.dispatchEvent(
    new PointerEvent(type, {
      bubbles: true,
      button: 0,
      buttons: type === 'pointerup' ? 0 : 1,
      clientX: 12,
      clientY,
      isPrimary: true,
      pointerId: 41,
      pointerType: 'mouse'
    })
  )
}

it('darkens turns for visible user or assistant messages, previews, and jumps by distance', async () => {
  const originalIntersectionObserver = window.IntersectionObserver
  const originalScrollIntoView = HTMLElement.prototype.scrollIntoView
  const observedTargets: Element[] = []
  let observerCallback: IntersectionObserverCallback | undefined

  class ControlledIntersectionObserver implements IntersectionObserver {
    readonly root = null
    readonly rootMargin = ''
    readonly thresholds = [0]

    constructor(callback: IntersectionObserverCallback) {
      observerCallback = callback
    }

    disconnect() {
      observedTargets.splice(0, observedTargets.length)
    }
    takeRecords(): IntersectionObserverEntry[] {
      return []
    }
    unobserve(target: Element) {
      const targetIndex = observedTargets.indexOf(target)
      if (targetIndex >= 0) observedTargets.splice(targetIndex, 1)
    }
    observe(target: Element) {
      observedTargets.push(target)
    }
  }

  window.IntersectionObserver = ControlledIntersectionObserver
  const scrollIntoView = vi.fn()
  HTMLElement.prototype.scrollIntoView = scrollIntoView

  try {
    await page.viewport(1024, 768)
    const screen = await render(<RailHarness />)
    await expect
      .element(screen.getByRole('navigation', { name: 'chat.turnNavigationLabel' }))
      .toBeVisible()

    expect(observedTargets).toHaveLength(8)
    expect(
      observedTargets.filter((target) => target.classList.contains('chat-message--user'))
    ).toHaveLength(4)
    expect(
      observedTargets.filter((target) => target.classList.contains('chat-message--assistant'))
    ).toHaveLength(4)

    const userOne = getRequiredElement('[data-message-id="user-1"]')
    const assistantOne = getRequiredElement('[data-message-id="assistant-user-1"]')
    const userThree = getRequiredElement('[data-message-id="user-3"]')
    observerCallback?.(
      [
        createIntersectionEntry(assistantOne, true, 1),
        createIntersectionEntry(userThree, true, 0.5)
      ],
      {} as IntersectionObserver
    )

    const firstButton = screen.getByRole('button', {
      name: /^chat.turnNavigationJumpToTurn 1(?: |$)/
    })
    const secondButton = screen.getByRole('button', {
      name: /^chat.turnNavigationJumpToTurn 2(?: |$)/
    })
    const thirdButton = screen.getByRole('button', {
      name: /^chat.turnNavigationJumpToTurn 3(?: |$)/
    })

    await expect.element(secondButton).toHaveAttribute('data-favorited', 'true')
    expect(firstButton.element()).not.toHaveAttribute('data-favorited')
    await expect.element(firstButton).toHaveAttribute('aria-current', 'location')
    await expect.element(thirdButton).toHaveAttribute('aria-current', 'location')
    expect(secondButton.element()).not.toHaveAttribute('aria-current')

    observerCallback?.(
      [createIntersectionEntry(userOne, true, 1), createIntersectionEntry(assistantOne, false, 0)],
      {} as IntersectionObserver
    )
    await expect.element(firstButton).toHaveAttribute('aria-current', 'location')

    observerCallback?.([createIntersectionEntry(userOne, false, 0)], {} as IntersectionObserver)
    await expect.element(firstButton).not.toHaveAttribute('aria-current')
    await expect.element(thirdButton).toHaveAttribute('aria-current', 'location')

    await secondButton.hover()
    const tooltip = screen.getByRole('tooltip')
    await expect.element(tooltip).toHaveTextContent('User 2')
    await expect.element(tooltip).toHaveTextContent('Assistant 2')
    const tooltipElement = tooltip.element() as HTMLElement
    const userPreview = tooltipElement.querySelector('strong')
    const assistantPreview = tooltipElement.querySelector('p')
    expect(userPreview).not.toBeNull()
    expect(assistantPreview).not.toBeNull()
    expect(window.getComputedStyle(userPreview as Element).whiteSpace).toBe('nowrap')
    expect(window.getComputedStyle(userPreview as Element).textOverflow).toBe('ellipsis')
    expect(window.getComputedStyle(assistantPreview as Element).webkitLineClamp).toBe('3')

    const buttons = [
      firstButton.element(),
      secondButton.element(),
      thirdButton.element(),
      screen.getByRole('button', { name: /^chat.turnNavigationJumpToTurn 4(?: |$)/ }).element()
    ] as HTMLButtonElement[]
    expect(
      buttons.map((button) => button.style.getPropertyValue('--conversation-turn-wave-progress'))
    ).toEqual(['0.7', '1', '0.7', '0.4'])

    await secondButton.click()
    expect(scrollIntoView).toHaveBeenCalledWith({
      behavior: 'smooth',
      block: 'start'
    })
    await expect
      .element(screen.getByRole('navigation', { name: 'chat.turnNavigationLabel' }))
      .toHaveAttribute('data-pointer-preview-suppressed', 'true')
    await expect
      .poll(() => document.querySelector('.conversation-turn-navigation__tooltip'))
      .toBeNull()

    const scrollRoot = getRequiredElement('.scroll-root')
    const farUser = getRequiredElement('[data-message-id="user-4"]')
    Object.defineProperty(scrollRoot, 'clientHeight', {
      configurable: true,
      value: 600
    })
    vi.spyOn(scrollRoot, 'getBoundingClientRect').mockReturnValue(createRect(0, 600))
    vi.spyOn(farUser, 'getBoundingClientRect').mockReturnValue(createRect(1800, 40))
    scrollIntoView.mockClear()

    await screen.getByRole('button', { name: /^chat.turnNavigationJumpToTurn 4(?: |$)/ }).click()
    expect(scrollIntoView).toHaveBeenCalledWith({
      behavior: 'auto',
      block: 'start'
    })
  } finally {
    window.IntersectionObserver = originalIntersectionObserver
    HTMLElement.prototype.scrollIntoView = originalScrollIntoView
  }
})

it('jumps once, continuously scrolls while dragging, and clears the preview on release', async () => {
  const originalScrollIntoView = HTMLElement.prototype.scrollIntoView
  const scrollIntoView = vi.fn()
  HTMLElement.prototype.scrollIntoView = scrollIntoView

  try {
    await page.viewport(1024, 768)
    const screen = await render(<RailHarness />)
    const navigation = screen.getByRole('navigation', {
      name: 'chat.turnNavigationLabel'
    })
    await expect.element(navigation).toBeVisible()

    const list = getRequiredElement('.conversation-turn-navigation__list') as HTMLDivElement
    const buttons = Array.from(
      list.querySelectorAll<HTMLButtonElement>('.conversation-turn-navigation__row')
    )
    expect(buttons).toHaveLength(4)

    const scrollRoot = getRequiredElement('.scroll-root')
    const scrollTo = vi.spyOn(scrollRoot, 'scrollTo').mockImplementation(() => undefined)
    Object.defineProperties(scrollRoot, {
      clientHeight: {
        configurable: true,
        value: 600
      },
      scrollHeight: {
        configurable: true,
        value: 2200
      }
    })
    vi.spyOn(scrollRoot, 'getBoundingClientRect').mockReturnValue(createRect(0, 600))
    items.forEach((item, index) => {
      vi.spyOn(
        getRequiredElement(`[data-message-id="${item.userMessageId}"]`),
        'getBoundingClientRect'
      ).mockReturnValue(createRect(index * 500, 40))
    })
    vi.spyOn(list, 'getBoundingClientRect').mockReturnValue({
      ...createRect(95, 45),
      left: 0,
      right: 36,
      width: 36
    })
    buttons.forEach((button, index) => {
      vi.spyOn(button, 'getBoundingClientRect').mockReturnValue({
        ...createRect(100 + index * 10, 10),
        left: 0,
        right: 36,
        width: 36
      })
    })
    vi.spyOn(list, 'setPointerCapture').mockImplementation(() => undefined)
    vi.spyOn(list, 'hasPointerCapture').mockReturnValue(true)
    vi.spyOn(list, 'releasePointerCapture').mockImplementation(() => undefined)

    dispatchPointerEvent(buttons[0], 'pointerdown', 105)
    dispatchPointerEvent(list, 'pointermove', 110)

    await expect.element(navigation).toHaveAttribute('data-scrubbing', 'true')
    await expect.element(buttons[0]).toHaveAttribute('data-previewed', 'true')
    await vi.waitFor(() =>
      expect(getComputedStyle(buttons[0].firstElementChild!).opacity).toBe('1')
    )
    const tooltip = screen.getByRole('tooltip')
    await expect.element(tooltip).toHaveTextContent('User 1')
    expect(scrollIntoView).toHaveBeenCalledTimes(1)
    expect(scrollIntoView).toHaveBeenLastCalledWith({
      behavior: 'auto',
      block: 'start'
    })

    dispatchPointerEvent(list, 'pointermove', 121)
    await expect.element(tooltip).toHaveTextContent('User 3')
    await expect.element(buttons[2]).toHaveAttribute('data-previewed', 'true')
    expect(scrollTo).toHaveBeenLastCalledWith({
      behavior: 'auto',
      top: 800
    })
    expect(scrollIntoView).toHaveBeenCalledTimes(1)

    dispatchPointerEvent(list, 'pointermove', 131)
    await expect.element(tooltip).toHaveTextContent('User 4')
    expect(scrollTo).toHaveBeenLastCalledWith({
      behavior: 'auto',
      top: 1300
    })
    expect(scrollTo).toHaveBeenCalledTimes(2)
    expect(scrollIntoView).toHaveBeenCalledTimes(1)

    dispatchPointerEvent(list, 'pointerup', 131)
    buttons[0].dispatchEvent(new MouseEvent('click', { bubbles: true, button: 0 }))

    await expect.element(navigation).not.toHaveAttribute('data-scrubbing')
    await expect.element(navigation).toHaveAttribute('data-pointer-preview-suppressed', 'true')
    await expect
      .poll(() => document.querySelector('.conversation-turn-navigation__tooltip'))
      .toBeNull()
    expect(scrollIntoView).toHaveBeenCalledTimes(1)
  } finally {
    HTMLElement.prototype.scrollIntoView = originalScrollIntoView
  }
})

it('keeps consecutive human and agent inputs individually reachable without sharing visible status', async () => {
  const revealMessage = vi.fn()
  const view = (visibleMessageIds: string[]) => (
    <ConsecutiveInputHarness
      onRevealMessage={revealMessage}
      visibleMessageIds={visibleMessageIds}
    />
  )
  const screen = await render(view(['human-one']))
  const rows = [
    ...screen.container.querySelectorAll<HTMLButtonElement>('.conversation-turn-navigation__row')
  ]
  expect(rows.map((row) => row.dataset.turnId)).toEqual([
    'earlier',
    'human-one',
    'agent-mail-one',
    'human-two'
  ])
  expect(rows.map((row) => row.dataset.source)).toEqual(['human', 'human', 'agent', 'human'])
  const visibleTurns = () =>
    rows
      .filter((row) => row.getAttribute('aria-current') === 'location')
      .map((row) => row.dataset.turnId)
  await expect.poll(visibleTurns).toEqual(['human-one'])
  await screen.rerender(view(['agent-mail-one']))
  await expect.poll(visibleTurns).toEqual(['agent-mail-one'])
  await screen.rerender(view(['shared-answer']))
  await expect.poll(visibleTurns).toEqual(['human-two'])
  await screen.rerender(view(['human-one', 'agent-mail-one', 'shared-answer']))
  await expect.poll(visibleTurns).toEqual(['human-one', 'agent-mail-one', 'human-two'])
  await screen.rerender(view(['pending-answer']))
  await expect.poll(visibleTurns).toEqual([])
  expect([...screen.container.querySelectorAll('.conversation-turn-navigation__row')]).toEqual(rows)

  await userEvent.hover(rows[2])
  const tooltip = screen.getByRole('tooltip')
  await expect.element(tooltip).toHaveTextContent('公式复核')
  expect(tooltip.element().querySelector('strong')?.textContent).toBe(
    '核对完成：参考 报告 得到结果。'
  )
  expect(tooltip.element().querySelector('p')?.textContent).toBe('计算与单位均已核对')
  expect(tooltip.element().textContent).not.toContain('review-agent')
  expect(tooltip.element().textContent).not.toContain('source-review-message')
  for (const row of rows.slice(1)) await userEvent.click(row)
  expect(revealMessage.mock.calls).toEqual([
    ['human-one', 'start'],
    ['agent-mail-one', 'start'],
    ['human-two', 'start']
  ])
})

it.each([
  { name: 'light', theme: classicLightTheme },
  { name: 'dark', theme: classicDarkTheme }
])(
  'fades only scrollable ends through real list scrolling and resize ($name)',
  async ({ name, theme }) => {
    const rootStyle = document.documentElement.getAttribute('style')
    for (const [variable, value] of Object.entries(
      getFrontendCssVariables(frontendConfig, theme)
    )) {
      document.documentElement.style.setProperty(variable, value)
    }
    try {
      await page.viewport(900, 680)
      const screen = await render(<ScrollableRailHarness count={3} height={560} />)
      expect(screen.container.querySelector('.conversation-turn-navigation')).toBeNull()
      await screen.rerender(<ScrollableRailHarness count={8} height={560} />)
      await screen.getByTestId('scroll-away').click()
      const panel = screen.getByTestId('scrollable-rail').element() as HTMLElement
      const list = panel.querySelector<HTMLElement>('.conversation-turn-navigation__list')!
      const assertEdges = async (up: boolean, down: boolean) => {
        await expect.poll(() => list.hasAttribute('data-can-scroll-up')).toBe(up)
        await expect.poll(() => list.hasAttribute('data-can-scroll-down')).toBe(down)
        // The flags must follow browser geometry, including automatic scroll clamping on deletion.
        expect(list.scrollTop > 1).toBe(up)
        expect(list.scrollHeight - list.clientHeight - list.scrollTop > 1).toBe(down)
        const style = getComputedStyle(list)
        expect(style.getPropertyValue('--turn-fade-top').trim()).toBe(up ? 'transparent' : 'black')
        expect(style.getPropertyValue('--turn-fade-bottom').trim()).toBe(
          down ? 'transparent' : 'black'
        )
        const maskColors = style.maskImage.match(/rgba?\([^)]*\)|transparent|black/g)
        expect(maskColors).toHaveLength(4)
        expect(maskColors?.[0]).toBe(up ? 'rgba(0, 0, 0, 0)' : 'rgb(0, 0, 0)')
        expect(maskColors?.[3]).toBe(down ? 'rgba(0, 0, 0, 0)' : 'rgb(0, 0, 0)')
      }
      const screenshot = (state: string) =>
        page.screenshot({
          element: panel,
          path: `../../../../../../.cache/conversation-turn-navigation-edges/${name}-${state}.png`
        })

      await assertEdges(false, false)
      expect(list.scrollHeight).toBeLessThanOrEqual(list.clientHeight)
      await screenshot('all-visible')

      await screen.rerender(<ScrollableRailHarness count={72} height={560} />)
      expect(panel.querySelector('.conversation-turn-navigation__list')).toBe(list)
      await assertEdges(false, true)
      expect(list.scrollHeight).toBeGreaterThan(list.clientHeight)
      await screenshot('top')

      list.scrollTo({ top: (list.scrollHeight - list.clientHeight) / 2, behavior: 'instant' })
      await assertEdges(true, true)
      await screenshot('middle')

      list.scrollTo({ top: list.scrollHeight, behavior: 'instant' })
      await assertEdges(true, false)
      await screenshot('bottom')

      const previousBottom = list.scrollTop
      await screen.rerender(<ScrollableRailHarness count={96} height={560} />)
      await assertEdges(true, true)
      expect(list.scrollTop).toBeCloseTo(previousBottom, 1)

      await screen.rerender(<ScrollableRailHarness count={8} height={560} />)
      await assertEdges(false, false)
      expect(list.scrollTop).toBe(0)

      await screen.rerender(<ScrollableRailHarness count={8} height={120} />)
      await assertEdges(false, true)
      list.scrollTo({ top: list.scrollHeight, behavior: 'instant' })
      await assertEdges(true, false)
      await screen.rerender(<ScrollableRailHarness count={8} height={560} />)
      await assertEdges(false, false)

      await screen.rerender(<ScrollableRailHarness count={3} height={560} />)
      expect(panel.querySelector('.conversation-turn-navigation')).toBeNull()
      await screen.rerender(<ScrollableRailHarness count={72} height={560} />)
      const recreated = panel.querySelector<HTMLElement>('.conversation-turn-navigation__list')!
      expect(recreated).not.toBe(list)
      await expect.poll(() => recreated.hasAttribute('data-can-scroll-down')).toBe(true)
      expect(recreated.hasAttribute('data-can-scroll-up')).toBe(false)
      expect(recreated.scrollTop).toBe(0)
    } finally {
      if (rootStyle === null) document.documentElement.removeAttribute('style')
      else document.documentElement.setAttribute('style', rootStyle)
    }
  }
)

it.each([
  { name: 'light', theme: classicLightTheme },
  { name: 'dark', theme: classicDarkTheme },
  { name: 'crab', theme: crabLightTheme }
])(
  'keeps source hierarchy through resting, pointer, favorite and keyboard states ($name)',
  async ({ name, theme }) => {
    const rootStyle = document.documentElement.getAttribute('style')
    for (const [name, value] of Object.entries(getFrontendCssVariables(frontendConfig, theme))) {
      document.documentElement.style.setProperty(name, value)
    }
    const revealMessage = vi.fn()
    try {
      await page.viewport(900, 680)
      const screen = await render(<DenseRailHarness onRevealMessage={revealMessage} />)
      const panel = screen.getByTestId('dense-rail').element() as HTMLElement
      const away = screen.getByTestId('away')
      const rows = Array.from(
        panel.querySelectorAll<HTMLButtonElement>('.conversation-turn-navigation__row')
      )
      const tick = (index: number) => rows[index].firstElementChild as HTMLElement
      const tickOpacity = (index: number) => Number(getComputedStyle(tick(index)).opacity)
      const tickWidth = (index: number) => tick(index).getBoundingClientRect().width
      const tickColor = (index: number) => getComputedStyle(tick(index)).backgroundColor
      const assertCentered = () => {
        const centers = rows.map((row, index) => {
          const rowRect = row.getBoundingClientRect()
          const tickRect = tick(index).getBoundingClientRect()
          const center = tickRect.left + tickRect.width / 2
          expect(center).toBeCloseTo(rowRect.left + rowRect.width / 2, 1)
          return center
        })
        expect(Math.max(...centers) - Math.min(...centers)).toBeLessThan(0.1)
      }
      const screenshot = (state: string) => {
        assertCentered()
        return page.screenshot({
          element: panel,
          path: `../../../../../../.cache/conversation-turn-navigation-piano/${name}-${state}.png`
        })
      }

      await away.click()
      expect(rows.filter((row) => row.dataset.source === 'human')).toHaveLength(4)
      expect(rows.filter((row) => row.dataset.source === 'agent')).toHaveLength(30)
      expect(rows.filter((row) => row.dataset.source === 'workflow')).toHaveLength(1)
      expect(rows.filter((row) => row.dataset.source === 'context')).toHaveLength(1)
      await vi.waitFor(() => {
        for (const [index, row] of rows.entries()) {
          expect(tickWidth(index)).toBeCloseTo(row.dataset.source === 'human' ? 12 : 6, 1)
        }
      })
      expect(tickWidth(0)).toBeGreaterThan(tickWidth(1))
      expect(tickOpacity(0)).toBeGreaterThan(tickOpacity(1))
      expect(tickOpacity(18)).toBeGreaterThan(tickOpacity(17))
      expect(tickOpacity(17)).toBeGreaterThan(tickOpacity(1))
      expect(tickWidth(3)).toBeCloseTo(tickWidth(0), 1)
      expect(tickWidth(25)).toBeCloseTo(tickWidth(1), 1)
      expect(tickColor(3)).toBe(tickColor(25))
      expect(tickColor(3)).not.toBe(tickColor(0))
      const idleHuman = { color: tickColor(0), width: tickWidth(0), opacity: tickOpacity(0) }
      const idleMail = { color: tickColor(16), width: tickWidth(16), opacity: tickOpacity(16) }
      const favoriteColor = tickColor(25)
      expect(rows[17]).toHaveAttribute('aria-current', 'location')
      expect(rows[18]).toHaveAttribute('aria-current', 'location')
      expect(rows.every((row) => row.getBoundingClientRect().height === 10)).toBe(true)
      await screenshot('rest')

      await userEvent.hover(rows[16])
      assertCentered()
      await vi.waitFor(() => expect(tickWidth(16)).toBeCloseTo(20, 1))
      expect(tickOpacity(16)).toBeGreaterThan(idleMail.opacity)
      expect(tickOpacity(16)).toBeLessThan(1)
      expect(tickColor(16)).toBe(idleMail.color)
      expect(getComputedStyle(rows[16]).backgroundColor).toBe('rgba(0, 0, 0, 0)')
      await expect.element(screen.getByRole('tooltip')).toHaveTextContent('第 17 轮')
      const source = screen
        .getByRole('tooltip')
        .element()
        .querySelector('.conversation-turn-navigation__tooltip-source')!
      expect(source.textContent).toContain('公式复核')
      expect(rows[16].getAttribute('aria-label')).toContain('公式复核')
      const hoveredMailWidth = tickWidth(16)
      await screenshot('mail-hover')

      await userEvent.hover(rows[18])
      await vi.waitFor(() => expect(tickWidth(18)).toBeCloseTo(26, 1))
      expect(tickWidth(18)).toBeGreaterThan(hoveredMailWidth)
      expect(tickColor(18)).toBe(idleHuman.color)
      expect(tickOpacity(18)).toBe(1)
      expect(getComputedStyle(rows[18]).backgroundColor).toBe('rgba(0, 0, 0, 0)')
      await screenshot('human-hover')

      await userEvent.hover(rows[25])
      await vi.waitFor(() => expect(tickWidth(25)).toBeCloseTo(hoveredMailWidth, 1))
      expect(tickColor(25)).toBe(favoriteColor)
      expect(tickOpacity(25)).toBe(1)
      expect(getComputedStyle(rows[25]).backgroundColor).toBe('rgba(0, 0, 0, 0)')
      await screenshot('favorite-mail-hover')

      await userEvent.hover(rows[12])
      await expect.element(screen.getByRole('tooltip')).toHaveTextContent('每日复核')
      await userEvent.hover(rows[28])
      expect(
        screen
          .getByRole('tooltip')
          .element()
          .querySelector('.conversation-turn-navigation__tooltip-source')?.textContent
      ).toBeTruthy()

      await away.click()
      await userEvent.keyboard('{Tab}'.repeat(18))
      expect(document.activeElement).toBe(rows[17])
      expect(rows[17].matches(':focus-visible')).toBe(true)
      await vi.waitFor(() => expect(tickWidth(17)).toBeCloseTo(hoveredMailWidth, 1))
      expect(tickColor(17)).toBe(idleMail.color)
      expect(tickOpacity(17)).toBeLessThan(1)
      await expect.element(screen.getByRole('tooltip')).toHaveTextContent('公式复核')
      expect(getComputedStyle(rows[17]).outlineWidth).toBe('2px')
      expect(getComputedStyle(rows[17]).outlineOffset).toBe('-1px')
      expect(getComputedStyle(rows[17]).backgroundColor).toBe('rgba(0, 0, 0, 0)')
      await screenshot('focus')
      await userEvent.keyboard('{Enter} ')
      expect(revealMessage.mock.calls).toEqual([
        ['dense-18', 'start'],
        ['dense-18', 'start']
      ])
      await userEvent.keyboard('{Escape}')
      expect(document.activeElement).not.toBe(rows[17])
      await vi.waitFor(() => expect(tickOpacity(0)).toBeCloseTo(idleHuman.opacity, 2))
      await expect
        .poll(() => document.querySelector('.conversation-turn-navigation__tooltip'))
        .toBeNull()

      await away.click()
      await userEvent.keyboard('{Tab}'.repeat(4))
      expect(document.activeElement).toBe(rows[3])
      expect(rows[3].matches(':focus-visible')).toBe(true)
      await vi.waitFor(() => expect(tickWidth(3)).toBeCloseTo(26, 1))
      expect(tickColor(3)).toBe(favoriteColor)
      expect(tickOpacity(3)).toBe(1)
      expect(getComputedStyle(rows[3]).backgroundColor).toBe('rgba(0, 0, 0, 0)')
      await screenshot('favorite-human-focus')

      panel.style.containerName = 'chat-messages'
      panel.style.containerType = 'inline-size'
      // Container queries measure the content box, excluding the fixture's text gutters.
      panel.style.width = '800px'
      panel.style.padding = '48px 32px'
      await expect.poll(() => rows[3].getBoundingClientRect().width).toBe(26)
      await screenshot('narrow-favorite-focus')
    } finally {
      if (rootStyle === null) document.documentElement.removeAttribute('style')
      else document.documentElement.setAttribute('style', rootStyle)
    }
  }
)
