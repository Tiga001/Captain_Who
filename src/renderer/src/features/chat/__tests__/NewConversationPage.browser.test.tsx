import { page } from 'vitest/browser'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import '../../../styles/global.css'

const LONG_PROJECT_NAME = 'LLMtest'.repeat(24)
const LONG_MIXED_PROJECT_NAME = '超长项目🚀'.repeat(32)
const SHORT_PROJECT_NAME = 'LLMtest'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) => (key === 'chat.projectTitle' ? '我们应该在{projectName}中做些什么？' : key)
  })
}))

vi.mock('../../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [
      {
        id: 'model-1',
        displayName: 'Model One',
        supportsImage: true,
        inputPrice: '0',
        outputPrice: '0',
        enabled: true
      }
    ]
  })
}))

vi.mock('../../../config/ProjectSettingsProvider', async () => {
  const { singleFolderProject } = await import('../../projects/__tests__/projectFixtures')
  return {
    useProjectSettings: () => ({
      projects: [
        singleFolderProject({
          id: 'project-long',
          name: LONG_PROJECT_NAME,
          path: '/workspace/long',
          createdAt: 1
        }),
        singleFolderProject({
          id: 'project-short',
          name: SHORT_PROJECT_NAME,
          path: '/workspace/short',
          createdAt: 2
        }),
        singleFolderProject({
          id: 'project-mixed',
          name: LONG_MIXED_PROJECT_NAME,
          path: '/workspace/mixed',
          createdAt: 3
        })
      ],
      openCreateProjectDialog: vi.fn(async () => null)
    })
  }
})

vi.mock('../../skills/useSkillCatalog', () => ({
  useSkillCatalog: () => ({
    refresh: vi.fn(),
    state: { status: 'idle' as const }
  })
}))

vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => vi.fn()
}))

vi.mock('../chatAttachments', () => ({
  buildAgentInputAttachments: (attachments: unknown[]) => attachments,
  composerAttachmentFromAgentAttachment: (attachment: unknown) => attachment,
  createComposerAttachmentsFromFiles: async () => [],
  loadComposerAttachmentImage: async () => undefined,
  loadComposerAttachmentPreview: async () => undefined,
  createAttachmentSummary: () => '',
  selectComposerAttachments: async () => [],
  stripAttachmentSummary: (content: string) => content
}))

const [
  { NewConversationPage },
  { createComposerDraft },
  { NEW_CONVERSATION_PROMPTS, getNewConversationPromptKeys }
] = await Promise.all([
  import('../NewConversationPage'),
  import('../../../app/chatMessageFactory'),
  import('../newConversationPrompts')
])

function renderPage(projectId: string | null, promptIndex = 0) {
  return render(
    <div data-testid="middle-panel" style={{ width: 480, height: 800 }}>
      <NewConversationPage
        draft={createComposerDraft({ modelId: 'model-1', projectId })}
        onDraftChange={vi.fn()}
        onSubmitMessage={vi.fn()}
        permissionModeAvailability={{ custom: true, full: true }}
        promptIndex={promptIndex}
      />
    </div>
  )
}

function expectContained(child: HTMLElement, parent: HTMLElement) {
  const childRect = child.getBoundingClientRect()
  const parentRect = parent.getBoundingClientRect()

  expect(childRect.left).toBeGreaterThanOrEqual(parentRect.left - 1)
  expect(childRect.right).toBeLessThanOrEqual(parentRect.right + 1)
}

function expectEllipsis(element: HTMLElement) {
  const style = getComputedStyle(element)

  expect(style.overflow).toBe('hidden')
  expect(style.textOverflow).toBe('ellipsis')
  expect(style.whiteSpace).toBe('nowrap')
  expect(element.scrollWidth).toBeGreaterThan(element.clientWidth)
}

afterEach(async () => {
  await page.viewport(1280, 720)
})

describe('NewConversationPage prompt pairing', () => {
  it.each(NEW_CONVERSATION_PROMPTS.map((_, promptIndex) => promptIndex))(
    'keeps root prompt title and placeholder paired for index %i',
    async (promptIndex) => {
      const screen = await renderPage(null, promptIndex)
      const prompt = getNewConversationPromptKeys(promptIndex)

      await expect
        .element(screen.getByRole('heading', { level: 1, name: prompt.titleKey }))
        .toBeVisible()
      await expect
        .element(screen.getByRole('textbox', { name: 'chat.inputAria' }))
        .toHaveAttribute('placeholder', prompt.placeholderKey)
    }
  )

  it('keeps the project-specific title and default placeholder instead of a root prompt pair', async () => {
    const screen = await renderPage('project-short', NEW_CONVERSATION_PROMPTS.length - 1)

    await expect
      .element(
        screen.getByRole('heading', {
          level: 1,
          name: new RegExp(`我们应该在\\s*${SHORT_PROJECT_NAME}\\s*中做些什么？`)
        })
      )
      .toBeVisible()
    await expect
      .element(screen.getByRole('textbox', { name: 'chat.inputAria' }))
      .toHaveAttribute('placeholder', 'chat.inputPlaceholder')
  })
})

describe('NewConversationPage project-name containment', () => {
  it('keeps the project row inside the composer rounded border', async () => {
    const screen = await renderPage(null)
    const composer = screen.container.querySelector<HTMLElement>('.chat-composer')!
    const projectRow = screen.container.querySelector<HTMLElement>('.chat-composer__project-row')!

    expectContained(projectRow, composer)
    expect(getComputedStyle(composer).overflow).toBe('visible')
    expect(getComputedStyle(projectRow).marginBottom).toBe('0px')
  })

  it('ellipsizes a long project name in the centered title and composer at the minimum middle width', async () => {
    await page.viewport(1440, 900)
    const screen = await renderPage('project-long')
    const middlePanel = screen.getByTestId('middle-panel').element() as HTMLElement
    const pageElement = screen
      .getByRole('region', { name: 'chat.newConversation' })
      .element() as HTMLElement
    const content = pageElement.querySelector<HTMLElement>('.new-conversation-page__content')!
    const heading = screen
      .getByRole('heading', {
        level: 1,
        name: new RegExp(`我们应该在\\s*${LONG_PROJECT_NAME}\\s*中做些什么？`)
      })
      .element() as HTMLElement
    const titleProjectName = heading.querySelector<HTMLElement>(
      '.new-conversation-page__project-name'
    )!
    const projectButton = screen
      .getByRole('button', {
        name: LONG_PROJECT_NAME
      })
      .element() as HTMLElement
    const composerProjectName = projectButton.querySelector<HTMLElement>('span')!

    expect(middlePanel.getBoundingClientRect().width).toBe(480)
    expect(content.getBoundingClientRect().width).toBeGreaterThan(400)
    expect(
      Math.abs(
        heading.getBoundingClientRect().left +
          heading.getBoundingClientRect().width / 2 -
          (content.getBoundingClientRect().left + content.getBoundingClientRect().width / 2)
      )
    ).toBeLessThanOrEqual(1)
    expect(getComputedStyle(heading).textAlign).toBe('center')
    expect(pageElement.scrollWidth).toBeLessThanOrEqual(pageElement.clientWidth + 1)
    expect(heading.scrollWidth).toBeLessThanOrEqual(heading.clientWidth + 1)
    expectContained(heading, pageElement)
    expectContained(titleProjectName, pageElement)
    expectContained(projectButton, pageElement)
    expectContained(composerProjectName, projectButton)

    expect(titleProjectName.title).toBe(LONG_PROJECT_NAME)
    expect(composerProjectName.title).toBe(LONG_PROJECT_NAME)
    expectEllipsis(titleProjectName)
    expectEllipsis(composerProjectName)
  })

  it('contains mixed CJK and emoji project names at the minimum middle width', async () => {
    await page.viewport(1440, 900)
    const screen = await renderPage('project-mixed')
    const pageElement = screen
      .getByRole('region', { name: 'chat.newConversation' })
      .element() as HTMLElement
    const heading = screen.getByRole('heading', { level: 1 }).element() as HTMLElement
    const titleProjectName = heading.querySelector<HTMLElement>(
      '.new-conversation-page__project-name'
    )!
    const projectButton = screen
      .getByRole('button', { name: LONG_MIXED_PROJECT_NAME })
      .element() as HTMLElement
    const composerProjectName = projectButton.querySelector<HTMLElement>('span')!

    expect(heading.textContent).toContain(LONG_MIXED_PROJECT_NAME)
    expectContained(heading, pageElement)
    expectContained(titleProjectName, pageElement)
    expectContained(projectButton, pageElement)
    expectContained(composerProjectName, projectButton)
    expectEllipsis(titleProjectName)
    expectEllipsis(composerProjectName)
  })

  it('keeps a short project name complete in both locations', async () => {
    await page.viewport(1440, 900)
    const screen = await renderPage('project-short')
    const heading = screen
      .getByRole('heading', {
        level: 1,
        name: new RegExp(`我们应该在\\s*${SHORT_PROJECT_NAME}\\s*中做些什么？`)
      })
      .element() as HTMLElement
    const titleProjectName = heading.querySelector<HTMLElement>(
      '.new-conversation-page__project-name'
    )!
    const projectButton = screen
      .getByRole('button', {
        name: SHORT_PROJECT_NAME
      })
      .element() as HTMLElement
    const composerProjectName = projectButton.querySelector<HTMLElement>('span')!

    expect(titleProjectName.textContent).toBe(SHORT_PROJECT_NAME)
    expect(composerProjectName.textContent).toBe(SHORT_PROJECT_NAME)
    expect(titleProjectName.title).toBe(SHORT_PROJECT_NAME)
    expect(composerProjectName.title).toBe(SHORT_PROJECT_NAME)
    expect(titleProjectName.scrollWidth).toBeLessThanOrEqual(titleProjectName.clientWidth + 1)
    expect(composerProjectName.scrollWidth).toBeLessThanOrEqual(composerProjectName.clientWidth + 1)
  })
})
