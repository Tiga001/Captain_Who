import type { CSSProperties } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { getTranslation } from '../../config/frontendTranslations'
import type { Translate } from '../../config/translationFormat'
import { singleFolderProject } from '../../features/projects/__tests__/projectFixtures'
import { MainPanelToolbar } from '../AppShellSupport'
import '../../styles/global.css'
import '../shell/MainPanelProjectCard.css'

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
  vi.restoreAllMocks()
  document.documentElement.removeAttribute('data-color-scheme')
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

function animationDuration(element: Element) {
  const timing = element.getAnimations()[0]?.effect?.getTiming()
  return typeof timing?.duration === 'number' ? timing.duration : 0
}

function translateX(element: Element) {
  const { transform } = getComputedStyle(element)
  const values = /matrix(?:3d)?\(([^)]+)\)/.exec(transform)?.[1]?.split(',')
  if (!values) return 0
  return Number(values.length === 16 ? values[12] : values[4])
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
    await expect
      .element(screen.getByRole('button', { name: 'project.openDetails' }))
      .not.toBeInTheDocument()
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
    await expect
      .element(screen.getByRole('button', { name: 'Captain Who' }))
      .not.toBeInTheDocument()
  })

  it('centers the theme-matching boat mark on the new conversation toolbar', async () => {
    const screen = await render(
      <div className="app-shell" style={shellStyle}>
        <MainPanelToolbar {...toolbarProps} />
      </div>
    )

    const boat = screen.getByRole('button', { name: 'Captain Who' }).element()
    const lightLogo = screen.container.querySelector<HTMLImageElement>(
      '.main-panel__brand-mark--light'
    )
    const darkLogo = screen.container.querySelector<HTMLImageElement>(
      '.main-panel__brand-mark--dark'
    )
    const titleSlot = screen.container.querySelector('.main-panel__title')
    const toolbar = screen.container.querySelector('.main-panel__toolbar')
    if (!lightLogo || !darkLogo || !titleSlot || !toolbar) {
      throw new Error('Missing toolbar brand mark')
    }
    expect(lightLogo.src).toContain('brand-mark-light')
    expect(darkLogo.src).toContain('brand-mark-dark')
    expect(getComputedStyle(lightLogo).backgroundColor).toBe('rgba(0, 0, 0, 0)')
    expect(getComputedStyle(titleSlot).overflow).toBe('hidden')
    expect(Math.abs(centerY(boat) - centerY(toolbar))).toBeLessThan(1)
    expect(
      (boat.getBoundingClientRect().left + boat.getBoundingClientRect().right) / 2
    ).toBeCloseTo(
      (titleSlot.getBoundingClientRect().left + titleSlot.getBoundingClientRect().right) / 2,
      0
    )

    document.documentElement.setAttribute('data-color-scheme', 'dark')
    expect(getComputedStyle(lightLogo).display).toBe('none')
    expect(getComputedStyle(darkLogo).display).not.toBe('none')
    document.documentElement.removeAttribute('data-color-scheme')
  })

  it('wobbles in place on the first clicks and speeds up consecutive taps', async () => {
    const screen = await render(
      <div className="app-shell" style={shellStyle}>
        <MainPanelToolbar {...toolbarProps} />
      </div>
    )

    const boat = screen.getByRole('button', { name: 'Captain Who' })
    await boat.click()
    await expect.element(boat).toHaveAttribute('data-motion', 'wobble')
    const firstDuration = animationDuration(boat.element())
    expect(firstDuration).toBeGreaterThan(400)
    expect(boat.element().getAnimations().length).toBeGreaterThan(0)

    await boat.click()
    await expect.element(boat).toHaveAttribute('data-motion', 'wobble')
    expect(animationDuration(boat.element())).toBeLessThan(firstDuration)
  })

  it('sails out of the title slot on the fifth click, ignores taps underway, then returns', async () => {
    const screen = await render(
      <div className="app-shell" style={shellStyle}>
        <MainPanelToolbar {...toolbarProps} />
      </div>
    )

    const boat = screen.getByRole('button', { name: 'Captain Who' })
    const boatNode = boat.element() as HTMLElement
    for (let click = 0; click < 4; click += 1) await boat.click()
    await expect.element(boat).toHaveAttribute('data-motion', 'wobble')

    await boat.click()
    await expect.element(boat).toHaveAttribute('data-motion', 'sail')
    boatNode.click()
    boatNode.click()
    expect(boatNode.getAttribute('data-motion')).toBe('sail')
    await expect.poll(() => translateX(boatNode)).toBeLessThan(-20)
    expect(boatNode.getAttribute('data-motion')).toBe('sail')

    await expect.poll(() => boatNode.getAttribute('data-motion'), { timeout: 4000 }).toBeNull()
    expect(Math.abs(translateX(boatNode))).toBeLessThan(1)
  })

  it('skips wobble and sail when the user prefers reduced motion', async () => {
    vi.spyOn(window, 'matchMedia').mockImplementation((query: string) => ({
      matches: query.includes('prefers-reduced-motion'),
      media: query,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
      onchange: null
    }))

    const screen = await render(
      <div className="app-shell" style={shellStyle}>
        <MainPanelToolbar {...toolbarProps} />
      </div>
    )

    const boat = screen.getByRole('button', { name: 'Captain Who' })
    for (let click = 0; click < 5; click += 1) await boat.click()
    expect(boat.element().getAttribute('data-motion')).toBeNull()
    expect(boat.element().getAnimations()).toHaveLength(0)
  })
})

describe('MainPanelToolbar project card', () => {
  const translate: Translate = (key) => getTranslation('en-US', key)
  const playground = {
    ...singleFolderProject({
      id: 'project-a',
      name: 'Playground',
      path: '/Users/me/Desktop/Playground'
    }),
    folders: [
      {
        id: 'folder-primary',
        path: '/Users/me/Desktop/Playground',
        alias: 'Playground',
        role: 'primary' as const,
        sortOrder: 0,
        createdAt: 1
      },
      {
        id: 'folder-notes',
        path: '/Users/me/Documents/Notes',
        alias: 'Notes',
        role: 'auxiliary' as const,
        sortOrder: 1,
        createdAt: 2
      }
    ]
  }

  it('opens project details from the title and reveals or edits from the card', async () => {
    const onEditProject = vi.fn()
    const onRevealFolder = vi.fn()
    const screen = await render(
      <div className="app-shell" style={shellStyle}>
        <MainPanelToolbar
          {...toolbarProps}
          t={translate}
          conversationActions={conversationActions()}
          projectCard={{
            conversationCount: 2,
            onEditProject,
            onRevealFolder,
            project: playground
          }}
          title="Open the polymer PDF"
        />
      </div>
    )

    const title = screen.getByRole('button', { name: 'Open the polymer PDF' }).element()
    const projectButton = screen.getByRole('button', { name: 'Open project details' })
    const menuButton = screen.getByRole('button', { name: 'More chat actions' }).element()
    await expect.element(projectButton).toBeVisible()
    expect(projectButton.element().getBoundingClientRect().right).toBeLessThanOrEqual(
      title.getBoundingClientRect().left + 1
    )
    expect(getComputedStyle(projectButton.element()).backgroundColor).toBe(
      getComputedStyle(menuButton).backgroundColor
    )
    expect(getComputedStyle(projectButton.element()).borderRadius).toBe(
      getComputedStyle(menuButton).borderRadius
    )

    await projectButton.click()
    await expect.element(screen.getByRole('dialog', { name: 'Playground' })).toBeVisible()
    await expect.element(screen.getByText('2 chats')).toBeVisible()
    expect(document.body.textContent).not.toContain('Pin project')
    await expect
      .element(screen.getByRole('button', { name: 'Show in folder: ~/Desktop/Playground' }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: 'Show in folder: ~/Documents/Notes' }))
      .toBeVisible()

    await screen.getByRole('button', { name: 'Show in folder: ~/Desktop/Playground' }).click()
    expect(onRevealFolder).toHaveBeenCalledWith('folder-primary')
    await expect.element(screen.getByRole('dialog', { name: 'Playground' })).not.toBeInTheDocument()

    await screen.getByRole('button', { name: 'Open project details' }).click()
    await screen.getByRole('button', { name: 'Edit project' }).click()
    expect(onEditProject).toHaveBeenCalledOnce()
    await expect.element(screen.getByRole('dialog', { name: 'Playground' })).not.toBeInTheDocument()
  })

  it('closes the project card when the conversation menu opens', async () => {
    const screen = await render(
      <div className="app-shell" style={shellStyle}>
        <MainPanelToolbar
          {...toolbarProps}
          t={translate}
          conversationActions={conversationActions()}
          projectCard={{
            conversationCount: 1,
            onEditProject: vi.fn(),
            onRevealFolder: vi.fn(),
            project: playground
          }}
          title="Trip notes"
        />
      </div>
    )

    await screen.getByRole('button', { name: 'Open project details' }).click()
    await expect.element(screen.getByRole('dialog', { name: 'Playground' })).toBeVisible()
    await screen.getByRole('button', { name: 'More chat actions' }).click()
    await expect.element(screen.getByRole('dialog', { name: 'Playground' })).not.toBeInTheDocument()
    await expect.element(screen.getByRole('menuitem', { name: 'Pin chat' })).toBeVisible()
  })
})
