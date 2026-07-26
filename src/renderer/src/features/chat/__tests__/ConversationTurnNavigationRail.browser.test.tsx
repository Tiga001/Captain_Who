import { useRef } from 'react'
import { expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
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

it('scrubs between turns while dragging and suppresses the release click', async () => {
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
    dispatchPointerEvent(list, 'pointermove', 125)

    await expect.element(navigation).toHaveAttribute('data-scrubbing', 'true')
    const tooltip = screen.getByRole('tooltip')
    await expect.element(tooltip).toHaveTextContent('User 3')
    await expect.element(tooltip).toHaveTextContent('Assistant 3')
    expect(scrollIntoView).toHaveBeenCalledTimes(1)
    expect(scrollIntoView).toHaveBeenLastCalledWith({
      behavior: 'auto',
      block: 'start'
    })

    dispatchPointerEvent(list, 'pointermove', 135)
    await expect.element(tooltip).toHaveTextContent('User 4')
    expect(scrollIntoView).toHaveBeenCalledTimes(2)
    expect(scrollIntoView).toHaveBeenLastCalledWith({
      behavior: 'auto',
      block: 'start'
    })

    dispatchPointerEvent(list, 'pointerup', 135)
    buttons[0].dispatchEvent(new MouseEvent('click', { bubbles: true, button: 0 }))

    await expect.element(navigation).not.toHaveAttribute('data-scrubbing')
    expect(scrollIntoView).toHaveBeenCalledTimes(2)
  } finally {
    HTMLElement.prototype.scrollIntoView = originalScrollIntoView
  }
})
