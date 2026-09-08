import { useState, type CSSProperties } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import { getFrontendTheme } from '../../../config/frontendTheme'
import type { ComposerCommandId } from '../components/ComposerCommands'
import '../../../styles/global.css'

const { translate, selectAttachments, executeCommand } = vi.hoisted(() => ({
  translate: (key: string) => key,
  selectAttachments: vi.fn(async () => []),
  executeCommand: vi.fn()
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: translate })
}))
vi.mock('../../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: Array.from({ length: 24 }, (_, index) => ({
      id: `model-${index}`,
      displayName: `Model ${index}`,
      providerModelId: `provider-${index}`,
      supportsImage: true,
      inputPrice: '0',
      outputPrice: '0',
      enabled: true
    }))
  })
}))
vi.mock('../../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({
    projects: Array.from({ length: 20 }, (_, index) => ({
      id: `project-${index}`,
      name: `Project ${index}`,
      path: `/workspace/${index}`,
      createdAt: index
    })),
    selectProjectDirectory: vi.fn()
  })
}))
vi.mock('../../skills/skillsClient', () => ({
  listSkills: async () => ({
    schemaVersion: 4,
    catalogRevision: 'catalog-1',
    diagnostics: [],
    truncated: false,
    skills: Array.from({ length: 20 }, (_, index) => ({
      activationScope: 'run',
      description: `Description ${index}`,
      id: `installed:user:skill-${index}`,
      location: `skills/${index}/SKILL.md`,
      name: `Skill ${index}`,
      revision: `revision-${index}`,
      source: { id: 'installed:user', kind: 'installed' },
      trust: 'untrusted'
    }))
  })
}))
vi.mock('../components/ImagePreview', () => ({ useImagePreview: () => vi.fn() }))
vi.mock('../chatAttachments', () => ({
  buildAgentInputAttachments: (attachments: unknown[]) => attachments,
  composerAttachmentFromAgentAttachment: (attachment: unknown) => attachment,
  createComposerAttachmentsFromFiles: async () => [],
  createAttachmentSummary: () => '',
  selectComposerAttachments: selectAttachments,
  stripAttachmentSummary: (content: string) => content
}))

const [{ NewConversationPage }, { createComposerDraft }] = await Promise.all([
  import('../NewConversationPage'),
  import('../../../app/chatMessageFactory')
])

function Workspace({
  width = 1120,
  height = 760,
  longDraft = true,
  halfHeight = true
}: {
  width?: number
  height?: number
  longDraft?: boolean
  halfHeight?: boolean
}) {
  const [bottomOpen, setBottomOpen] = useState(halfHeight)
  const [covered, setCovered] = useState(false)
  const [draft, setDraft] = useState(() =>
    createComposerDraft({
      modelId: 'model-0',
      projectId: 'project-0',
      message: longDraft ? Array.from({ length: 20 }, (_, i) => `Draft line ${i}`).join('\n') : '',
      attachments: longDraft
        ? [
            {
              id: 'attachment-1',
              kind: 'file',
              name: 'notes.txt',
              mimeType: 'text/plain',
              sizeBytes: 4,
              encoding: 'base64',
              data: 'dGVzdA=='
            }
          ]
        : [],
      skills: longDraft ? [{ id: 'installed:user:skill-0', revision: 'revision-0' }] : []
    })
  )
  return (
    <div
      className="app-shell"
      data-left-open="true"
      data-right-open="true"
      data-right-maximized={String(covered)}
      data-bottom-open={String(bottomOpen)}
      style={
        {
          width,
          height,
          '--left-panel-width': '180px',
          '--right-panel-width': '280px',
          '--bottom-panel-height': bottomOpen ? `${height / 2}px` : '0px'
        } as CSSProperties
      }
    >
      <aside className="side-panel side-panel--left">
        <button type="button" onClick={() => setBottomOpen((value) => !value)}>
          toggle bottom
        </button>
        <button type="button" onClick={() => setCovered((value) => !value)}>
          toggle cover
        </button>
        <button type="button">outside</button>
      </aside>
      <main className="main-panel" inert={covered} aria-hidden={covered || undefined}>
        <div className="main-panel__toolbar">New conversation</div>
        <div className="main-panel__surface">
          <NewConversationPage
            commands={(
              ['new', 'compact', 'fork', 'usage', 'pin', 'rename', 'archive'] as Exclude<
                ComposerCommandId,
                'capabilities'
              >[]
            ).map((id) => ({
              id,
              label: id === 'new' ? 'New task' : id,
              description: 'Run this command',
              execute: executeCommand
            }))}
            draft={draft}
            onDraftChange={setDraft}
            onSubmitMessage={vi.fn()}
            permissionModeAvailability={{ custom: true, full: true }}
            promptIndex={0}
          />
        </div>
      </main>
      <aside className="side-panel side-panel--right">Review</aside>
      <aside className="side-panel side-panel--bottom">Terminal</aside>
    </div>
  )
}

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
function element(selector: string): HTMLElement {
  const node = document.querySelector<HTMLElement>(selector)
  if (!node) throw new Error(`Missing ${selector}`)
  return node
}
async function scrollToBottom() {
  const scroller = element('.new-conversation-page')
  scroller.scrollTop = scroller.scrollHeight
  await frame()
  return scroller
}
async function expectFloatingMenu(selector: string) {
  await expect.poll(() => document.querySelector(selector)).not.toBeNull()
  await expect
    .poll(() => {
      const menu = element(selector)
      const box = menu.getBoundingClientRect()
      return (
        !menu.closest('.new-conversation-page') &&
        box.width > 0 &&
        box.height > 0 &&
        box.top >= 0 &&
        box.left >= 0 &&
        box.right <= window.innerWidth + 1 &&
        box.bottom <= window.innerHeight + 1
      )
    })
    .toBe(true)
}

let previousRootStyle: string | null
beforeEach(async () => {
  vi.clearAllMocks()
  previousRootStyle = document.documentElement.getAttribute('style')
  const theme = getFrontendTheme('classic-light')
  for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, theme.tokens))) {
    document.documentElement.style.setProperty(key, value)
  }
  await page.viewport(1120, 760)
})
afterEach(async () => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
  await page.viewport(1280, 720)
})

describe('New conversation with a half-height bottom panel', () => {
  it.each([
    { width: 1120, height: 760, longDraft: false },
    { width: 920, height: 640, longDraft: true }
  ])(
    'scrolls all content into reach at $width × $height without moving fixed panels',
    async (size) => {
      await page.viewport(size.width, size.height)
      await render(<Workspace {...size} />)
      const scroller = element('.new-conversation-page')
      const fixedSelectors = [
        '.main-panel__toolbar',
        '.side-panel--left',
        '.side-panel--right',
        '.side-panel--bottom'
      ]
      const boxes = fixedSelectors.map((selector) =>
        element(selector).getBoundingClientRect().toJSON()
      )
      expect(getComputedStyle(scroller).overflowY).toBe('auto')
      expect(scroller.scrollHeight).toBeGreaterThan(scroller.clientHeight)
      expect(scroller.scrollWidth).toBeLessThanOrEqual(scroller.clientWidth + 1)
      expect(element('h1').getBoundingClientRect().top).toBeGreaterThanOrEqual(
        scroller.getBoundingClientRect().top
      )

      await scrollToBottom()
      expect(scroller.scrollTop).toBeGreaterThan(0)
      const project = element('.chat-composer__project-row').getBoundingClientRect()
      expect(project.top).toBeGreaterThanOrEqual(scroller.getBoundingClientRect().top)
      expect(project.bottom).toBeLessThanOrEqual(scroller.getBoundingClientRect().bottom)
      expect(
        fixedSelectors.map((selector) => element(selector).getBoundingClientRect().toJSON())
      ).toEqual(boxes)
      expect(document.scrollingElement?.scrollTop).toBe(0)

      await page.getByRole('button', { name: 'toggle bottom' }).click()
      await expect.poll(() => scroller.clientHeight).toBe(size.height - 52)
      expect(
        element('.chat-composer__project-row').getBoundingClientRect().bottom
      ).toBeLessThanOrEqual(scroller.getBoundingClientRect().bottom)
    }
  )

  it('keeps a short draft centered without a scrollbar when height is sufficient', async () => {
    await page.viewport(1440, 900)
    await render(<Workspace width={1440} height={900} halfHeight={false} longDraft={false} />)
    const scroller = element('.new-conversation-page')
    const content = element('.new-conversation-page__content')
    expect(scroller.scrollHeight).toBe(scroller.clientHeight)
    expect(getComputedStyle(scroller).display).toBe('grid')
    expect(content.getBoundingClientRect().top).toBeGreaterThan(
      scroller.getBoundingClientRect().top + 100
    )
    expect(content.getBoundingClientRect().left + content.clientWidth / 2).toBeCloseTo(
      scroller.getBoundingClientRect().left + scroller.clientWidth / 2,
      0
    )
  })

  it('keeps project search and selection clickable beyond the scroller bounds', async () => {
    await render(<Workspace />)
    await scrollToBottom()
    await page.getByRole('button', { name: 'Project 0', exact: true }).click()
    await expectFloatingMenu('.composer-project-menu')
    await page.getByPlaceholder('project.searchProject').fill('Project 19')
    await page.getByRole('option', { name: 'Project 19', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: 'Project 19', exact: true }))
      .toBeVisible()
    expect(document.querySelector('.composer-project-menu')).toBeNull()
  })

  it('preserves model menu focus, internal scrolling, selection, and Escape', async () => {
    await render(<Workspace />)
    await scrollToBottom()
    const trigger = page.getByRole('button', { name: 'chat.selectModel' })
    await trigger.click()
    await expectFloatingMenu('.composer-model-menu')
    await expect
      .poll(() => element('.composer-model-menu').contains(document.activeElement))
      .toBe(true)
    await userEvent.keyboard('{End}')
    await expect.poll(() => document.activeElement?.textContent).toContain('Model 23')
    await expectFloatingMenu('.composer-model-menu')
    await userEvent.keyboard('{Enter}')
    await expect.element(trigger).toHaveTextContent('Model 23')
    await expect.poll(() => document.activeElement).toBe(trigger.element())
    await trigger.click()
    await userEvent.keyboard('{Escape}')
    expect(document.querySelector('.composer-model-menu')).toBeNull()
    await expect.poll(() => document.activeElement).toBe(trigger.element())
    await trigger.click()
    await page.getByRole('option', { name: 'Model 3 configuration.image', exact: true }).click()
    await expect.element(trigger).toHaveTextContent('Model 3')
  })

  it('supports attachment actions and permission choices inside floating menus', async () => {
    await render(<Workspace />)
    await scrollToBottom()
    await page.getByRole('button', { name: 'chat.addContext' }).click()
    await expectFloatingMenu('.composer-add-menu')
    await page.getByRole('menuitem', { name: 'chat.addFile' }).click()
    expect(selectAttachments).toHaveBeenCalled()
    await page.getByRole('button', { name: /chat.permission/ }).click()
    await expectFloatingMenu('.composer-permission-menu')
    await page.getByRole('option', { name: 'chat.defaultPermission', exact: true }).click()
    expect(document.querySelector('.composer-permission-menu')).toBeNull()
  })

  it('supports skill search and selection without treating the portal as an outside click', async () => {
    await render(<Workspace />)
    await scrollToBottom()
    await page.getByRole('button', { name: 'chat.addContext' }).click()
    await page.getByRole('menuitem', { name: 'chat.skills' }).click()
    await expectFloatingMenu('.composer-skill-menu')
    await page.getByRole('textbox', { name: 'chat.searchSkills' }).fill('Skill 19')
    await page.getByRole('button', { name: /^Skill 19/ }).click()
    await expectFloatingMenu('.composer-skill-menu')
    await page.getByRole('textbox', { name: 'chat.searchSkills' }).click()
    await userEvent.keyboard('{Escape}')
    expect(document.querySelector('.composer-skill-menu')).toBeNull()
    await expect
      .element(page.getByRole('button', { name: /^chat.removeSkill Skill 19/ }))
      .toBeVisible()
  })

  it('keeps slash command options reachable and executes without submitting the draft', async () => {
    await render(<Workspace longDraft={false} />)
    await scrollToBottom()
    await page.getByRole('textbox', { name: 'chat.inputAria' }).click()
    await userEvent.keyboard('/')
    await expectFloatingMenu('.composer-commands')
    const scroller = element('.new-conversation-page')
    const scrollTop = scroller.scrollTop
    const inputBox = element('textarea').getBoundingClientRect().toJSON()
    await userEvent.keyboard('{ArrowUp}')
    await expect
      .element(page.getByRole('option', { name: /^archive/ }))
      .toHaveAttribute('aria-selected', 'true')
    expect(scroller.scrollTop).toBe(scrollTop)
    expect(element('textarea').getBoundingClientRect().toJSON()).toEqual(inputBox)
    await userEvent.keyboard('{ArrowDown}')
    await page.getByRole('option', { name: /New task/ }).click()
    expect(executeCommand).toHaveBeenCalledOnce()
    expect(document.querySelector('.composer-commands')).toBeNull()
  })

  it('dismisses menus on outside click, when their anchor scrolls out, and when the main page is covered', async () => {
    await render(<Workspace />)
    const scroller = await scrollToBottom()
    await page.getByRole('button', { name: 'chat.addContext' }).click()
    await expectFloatingMenu('.composer-add-menu')
    await page.getByRole('button', { name: 'outside', exact: true }).click()
    expect(document.querySelector('.composer-add-menu')).toBeNull()
    await page.getByRole('button', { name: 'Project 0', exact: true }).click()
    await expectFloatingMenu('.composer-project-menu')
    scroller.scrollTop = 0
    await expect.poll(() => document.querySelector('.composer-project-menu')).toBeNull()
    await scrollToBottom()
    await page.getByRole('button', { name: 'chat.selectModel' }).click()
    await expectFloatingMenu('.composer-model-menu')
    // Bypass pointer dismissal to exercise hiding a mounted workspace independently.
    ;(page.getByRole('button', { name: 'toggle cover' }).element() as HTMLButtonElement).click()
    await expect.poll(() => document.querySelector('.composer-model-menu')).toBeNull()
  })
})
