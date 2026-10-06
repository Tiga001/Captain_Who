import { useRef, type CSSProperties } from 'react'
import { expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import { ConversationTurnNavigationRail } from '../components/ConversationTurnNavigationRail'
import type { ConversationTurnNavigationItem } from '../conversationTurnNavigation'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => key
  })
}))

const items: ConversationTurnNavigationItem[] = Array.from({ length: 4 }, (_, index) => ({
  favorited: index === 1,
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

const denseItems: ConversationTurnNavigationItem[] = Array.from({ length: 36 }, (_, index) => ({
  favorited: index === 3 || index === 25,
  id: `dense-${index + 1}`,
  userMessageId: `dense-${index + 1}`,
  userPreview: `第 ${index + 1} 轮：复核报告中的关键结论`,
  assistantPreview: '已经复核相关证据，并将结果整理到项目文件中。'
}))

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
        items={denseItems}
        messageTurnIds={denseItems.map((item) => ({ id: item.id, role: 'user' }))}
        onRevealMessage={onRevealMessage}
        scrollContainerRef={scrollContainerRef}
        visibleMessageIds={new Set(['dense-18', 'dense-19'])}
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
      name: 'chat.turnNavigationJumpToTurn 1'
    })
    const secondButton = screen.getByRole('button', {
      name: 'chat.turnNavigationJumpToTurn 2'
    })
    const thirdButton = screen.getByRole('button', {
      name: 'chat.turnNavigationJumpToTurn 3'
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
      screen.getByRole('button', { name: 'chat.turnNavigationJumpToTurn 4' }).element()
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

    await screen.getByRole('button', { name: 'chat.turnNavigationJumpToTurn 4' }).click()
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

it.each([false, true])(
  'keeps a dense rail quiet at rest and clear for pointer and keyboard navigation (dark=%s)',
  async (dark) => {
    const rootStyle = document.documentElement.getAttribute('style')
    for (const [name, value] of Object.entries(
      getFrontendCssVariables(frontendConfig, dark ? classicDarkTheme : classicLightTheme)
    )) {
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
      const tickOpacity = (index: number) =>
        getComputedStyle(rows[index].firstElementChild!).opacity
      const screenshot = (state: string) =>
        page.screenshot({
          element: panel,
          path: `../../../../../../.cache/conversation-turn-navigation/${dark ? 'dark' : 'light'}-${state}.png`
        })

      await away.click()
      await vi.waitFor(() => expect(tickOpacity(0)).toBe('0.2'))
      expect(tickOpacity(3)).toBe('0.62')
      expect(tickOpacity(17)).toBe('0.66')
      expect(rows[17]).toHaveAttribute('aria-current', 'location')
      expect(rows[18]).toHaveAttribute('aria-current', 'location')
      expect(rows.every((row) => row.getBoundingClientRect().height === 10)).toBe(true)
      await screenshot('rest')

      await userEvent.hover(rows[16])
      await vi.waitFor(() => expect(tickOpacity(16)).toBe('1'))
      expect(tickOpacity(0)).toBe('0.4')
      expect(tickOpacity(17)).toBe('0.8')
      expect(tickOpacity(3)).toBe('0.86')
      expect(getComputedStyle(rows[16]).backgroundColor).toBe('rgba(0, 0, 0, 0)')
      await expect.element(screen.getByRole('tooltip')).toHaveTextContent('第 17 轮')
      await screenshot('hover')

      await away.click()
      await userEvent.keyboard('{Tab}'.repeat(18))
      expect(document.activeElement).toBe(rows[17])
      expect(rows[17].matches(':focus-visible')).toBe(true)
      await vi.waitFor(() => expect(tickOpacity(17)).toBe('1'))
      expect(tickOpacity(0)).toBe('0.4')
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
      await vi.waitFor(() => expect(tickOpacity(0)).toBe('0.2'))
      await expect
        .poll(() => document.querySelector('.conversation-turn-navigation__tooltip'))
        .toBeNull()
    } finally {
      if (rootStyle === null) document.documentElement.removeAttribute('style')
      else document.documentElement.setAttribute('style', rootStyle)
    }
  }
)
