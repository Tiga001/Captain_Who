import { useReducer } from 'react'
import { userEvent } from 'vitest/browser'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { RightSidebarTabStrip } from '../RightSidebarTabStrip'
import { reduceRightSidebarPlatform } from '../rightSidebarPlatformState'
import type { RightSidebarPlatformState } from '../rightSidebarPlatformState'
import '../../../styles/global.css'
import '../RightSidebar.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const initialState: RightSidebarPlatformState = {
  activePageId: 'first',
  pages: [
    { id: 'first', moduleId: 'terminal', title: 'Terminal Alpha' },
    { id: 'second', moduleId: 'terminal', title: 'Terminal Beta' }
  ]
}

interface PointerRecord {
  isTrusted: boolean
  targetClassName: string | undefined
  type: string
}

function Fixture({
  events,
  onClose,
  onActivate
}: {
  events: PointerRecord[]
  onClose: (id: string) => void
  onActivate: (id: string) => void
}) {
  const [state, dispatch] = useReducer(reduceRightSidebarPlatform, initialState)
  return (
    <div
      style={{ height: 200, padding: 40, width: 650 }}
      onPointerDownCapture={(event) =>
        events.push({
          isTrusted: event.isTrusted,
          targetClassName: (event.target as Element).closest('button')?.className,
          type: event.type
        })
      }
      onPointerUpCapture={(event) =>
        events.push({
          isTrusted: event.isTrusted,
          targetClassName: (event.target as Element).closest('button')?.className,
          type: event.type
        })
      }
    >
      <RightSidebarTabStrip
        activePageId={state.activePageId}
        availableModules={[]}
        isMenuOpen={false}
        modules={[]}
        onActivatePage={(pageId) => {
          onActivate(pageId)
          dispatch({ type: 'activate', pageId })
        }}
        onClosePage={(pageId) => {
          onClose(pageId)
          dispatch({ type: 'close', pageId })
        }}
        onMenuOpenChange={() => undefined}
        onOpenModule={() => undefined}
        pages={state.pages}
        t={(key) => key}
      />
      <textarea aria-label="Terminal input" style={{ marginTop: 20 }} />
    </div>
  )
}

describe('RightSidebarTabStrip close targets', () => {
  it('closes on the first fast edge press even when pointerup happens after the hover animation', async () => {
    const events: PointerRecord[] = []
    const onClose = vi.fn()
    const onActivate = vi.fn()
    const screen = await render(
      <Fixture events={events} onClose={onClose} onActivate={onActivate} />
    )
    const input = screen.getByRole('textbox', { name: 'Terminal input' })
    await input.hover()
    input.element().focus()
    const shell = screen.getByRole('tab', { name: 'Terminal Alpha' }).element().parentElement!
    const close = shell.querySelector<HTMLElement>('.right-sidebar__tab-close')!
    // Let any prior hover settle while the pointer remains outside the tabs.
    await expect.poll(() => getComputedStyle(close).visibility).toBe('hidden')
    const shellRect = shell.getBoundingClientRect()

    // Click the stable tab shell at the old close circle's outer edge. `force` skips
    // Playwright's stability/hit-target waiting, and `delay` separates real down/up
    // across the former 120ms scale animation without first hovering the close button.
    await userEvent.click(shell, {
      delay: 180,
      force: true,
      position: { x: shellRect.width - 6.3, y: shellRect.height / 2 }
    })

    expect(onClose).toHaveBeenCalledExactlyOnceWith('first')
    expect(onActivate).not.toHaveBeenCalled()
    expect(events).toEqual([
      { isTrusted: true, targetClassName: 'right-sidebar__tab-close', type: 'pointerdown' },
      { isTrusted: true, targetClassName: 'right-sidebar__tab-close', type: 'pointerup' }
    ])
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
  })

  it('closes an inactive tab directly without activating it first', async () => {
    const events: PointerRecord[] = []
    const onClose = vi.fn()
    const onActivate = vi.fn()
    const screen = await render(
      <Fixture events={events} onClose={onClose} onActivate={onActivate} />
    )
    await screen.getByRole('textbox', { name: 'Terminal input' }).hover()
    const shell = screen.getByRole('tab', { name: 'Terminal Beta' }).element().parentElement!
    const rect = shell.getBoundingClientRect()

    await userEvent.click(shell, {
      force: true,
      position: { x: rect.width - 13, y: rect.height / 2 }
    })

    expect(onClose).toHaveBeenCalledExactlyOnceWith('second')
    expect(onActivate).not.toHaveBeenCalled()
    await expect
      .element(screen.getByRole('tab', { name: 'Terminal Alpha' }))
      .toHaveAttribute('aria-selected', 'true')
    expect(events).toHaveLength(2)
    expect(
      events.every(
        (event) => event.isTrusted && event.targetClassName === 'right-sidebar__tab-close'
      )
    ).toBe(true)
  })

  it('keeps a fixed 24px hit area around the original 14px visual and supports keyboard closing', async () => {
    const screen = await render(<Fixture events={[]} onClose={vi.fn()} onActivate={vi.fn()} />)
    const tab = screen.getByRole('tab', { name: 'Terminal Beta' }).element()
    const close = tab.parentElement!.querySelector<HTMLButtonElement>('.right-sidebar__tab-close')!
    const icon = close.querySelector<HTMLElement>('.right-sidebar__tab-close-icon')!
    tab.focus()
    expect(close.getBoundingClientRect().width).toBe(24)
    expect(close.getBoundingClientRect().height).toBe(24)
    expect(icon.getBoundingClientRect().width).toBe(14)
    expect(icon.getBoundingClientRect().height).toBe(14)
    expect(getComputedStyle(close).transform).toBe('none')
    expect(getComputedStyle(close).pointerEvents).toBe('auto')
    close.focus()
    await userEvent.keyboard('{Enter}')
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
  })
})
