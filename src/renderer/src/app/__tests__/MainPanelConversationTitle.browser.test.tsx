import type { CSSProperties } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import type { Translate } from '../../config/translationFormat'
import { MainPanelToolbar } from '../AppShellSupport'
import '../../styles/global.css'

vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

const t: Translate = (key) => key
let previousRootStyle: string | null

beforeEach(() => {
  previousRootStyle = document.documentElement.getAttribute('style')
  for (const [key, value] of Object.entries(getFrontendCssVariables())) {
    document.documentElement.style.setProperty(key, value)
  }
})

afterEach(() => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
})

const toolbarProps = {
  bottomOpen: false,
  hasUnreadConversations: false,
  leftOpen: true,
  onToggleBottomPanel: () => undefined,
  onToggleLeftSidebar: () => undefined,
  onToggleRightSidebar: () => undefined,
  rightOpen: false,
  t
}

const shellStyle = {
  height: 400,
  position: 'relative',
  width: 800
} as CSSProperties

function centerY(element: Element) {
  const box = element.getBoundingClientRect()
  return (box.top + box.bottom) / 2
}

function conversationActions(
  overrides: {
    onArchive?: () => void
    onCommitTitle?: (title: string) => void
    onRename?: () => void
    onTogglePin?: () => void
  } = {}
) {
  return {
    conversationId: 'conversation-1',
    isPinned: false,
    onArchive: overrides.onArchive ?? vi.fn(),
    onCommitTitle: overrides.onCommitTitle ?? vi.fn(),
    onRename: overrides.onRename ?? vi.fn(),
    onTogglePin: overrides.onTogglePin ?? vi.fn()
  }
}

describe('MainPanelToolbar conversation menu', () => {
  it('opens the same conversation actions as the sidebar without mark unread', async () => {
    const onArchive = vi.fn()
    const onCommitTitle = vi.fn()
    const onRename = vi.fn()
    const onTogglePin = vi.fn()
    const screen = await render(
      <div className="app-shell" style={shellStyle}>
        <MainPanelToolbar
          {...toolbarProps}
          conversationActions={conversationActions({
            onArchive,
            onCommitTitle,
            onRename,
            onTogglePin
          })}
          title="Trip notes"
        />
      </div>
    )

    const title = screen.getByRole('button', { name: 'Trip notes' }).element()
    const menuButton = screen.getByRole('button', { name: 'sidebar.moreConversationActions' })
    const toolbar = screen.container.querySelector('.main-panel__toolbar')
    if (!toolbar) throw new Error('Missing toolbar')
    await expect.element(menuButton).toBeVisible()
    expect(menuButton.element().getBoundingClientRect().left).toBeGreaterThanOrEqual(
      title.getBoundingClientRect().right
    )
    expect(getComputedStyle(title).fontSize).toBe(
      getComputedStyle(document.documentElement)
        .getPropertyValue('--mc-font-size-chat-message')
        .trim()
    )
    expect(Math.abs(centerY(title) - centerY(toolbar))).toBeLessThan(1)
    expect(Math.abs(centerY(menuButton.element()) - centerY(toolbar))).toBeLessThan(1)

    await menuButton.click()
    await expect
      .element(screen.getByRole('menuitem', { name: 'conversation.pinConversation' }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('menuitem', { name: 'conversation.renameConversation' }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('menuitem', { name: 'conversation.archiveConversation' }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('menuitem', { name: 'conversation.markUnread' }))
      .not.toBeInTheDocument()
    expect(
      getComputedStyle(
        screen.getByRole('menuitem', { name: 'conversation.pinConversation' }).element()
      ).borderTopWidth
    ).toBe('0px')

    await screen.getByRole('menuitem', { name: 'conversation.renameConversation' }).click()
    expect(onRename).toHaveBeenCalledOnce()
    expect(onCommitTitle).not.toHaveBeenCalled()
    expect(onArchive).not.toHaveBeenCalled()
    expect(onTogglePin).not.toHaveBeenCalled()
    await expect.element(screen.getByRole('button', { name: 'Trip notes' })).toBeVisible()
  })

  it('renames the conversation inline from the title and ignores empty or cancelled edits', async () => {
    const onCommitTitle = vi.fn()
    const onRename = vi.fn()
    const screen = await render(
      <div className="app-shell" style={shellStyle}>
        <MainPanelToolbar
          {...toolbarProps}
          conversationActions={conversationActions({ onCommitTitle, onRename })}
          title="Trip notes"
        />
      </div>
    )

    await screen.getByRole('button', { name: 'Trip notes' }).click()
    const editor = screen.getByRole('textbox', { name: 'conversation.renameTitle' })
    await expect.element(editor).toBeVisible()
    await editor.fill('Weekend plans')
    await userEvent.keyboard('{Enter}')
    expect(onCommitTitle).toHaveBeenCalledOnce()
    expect(onCommitTitle).toHaveBeenCalledWith('Weekend plans')
    expect(onRename).not.toHaveBeenCalled()

    await screen.rerender(
      <div className="app-shell" style={shellStyle}>
        <MainPanelToolbar
          {...toolbarProps}
          conversationActions={conversationActions({ onCommitTitle, onRename })}
          title="Weekend plans"
        />
      </div>
    )

    await screen.getByRole('button', { name: 'Weekend plans' }).click()
    await screen.getByRole('textbox', { name: 'conversation.renameTitle' }).fill('   ')
    await userEvent.keyboard('{Enter}')
    expect(onCommitTitle).toHaveBeenCalledOnce()
    await expect.element(screen.getByRole('button', { name: 'Weekend plans' })).toBeVisible()

    await screen.getByRole('button', { name: 'Weekend plans' }).click()
    await screen.getByRole('textbox', { name: 'conversation.renameTitle' }).fill('Discarded')
    await userEvent.keyboard('{Escape}')
    expect(onCommitTitle).toHaveBeenCalledOnce()
    expect(onRename).not.toHaveBeenCalled()
    await expect.element(screen.getByRole('button', { name: 'Weekend plans' })).toBeVisible()
  })

  it('keeps the title-only toolbar without a conversation menu trigger', async () => {
    const screen = await render(<MainPanelToolbar {...toolbarProps} title="Trip notes" />)

    await expect.element(screen.getByRole('heading', { name: 'Trip notes' })).toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: 'sidebar.moreConversationActions' }))
      .not.toBeInTheDocument()
  })
})
