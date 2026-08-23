import type { CSSProperties } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import { ScheduledPage } from '../ScheduledPage'
import {
  makeAutomationRun,
  makeAutomationTask,
  testConversation,
  testModel
} from './automationUiFixtures'
import '../ScheduledPage.css'

const service = vi.hoisted(() => ({
  create: vi.fn(),
  update: vi.fn(),
  setEnabled: vi.fn(),
  runNow: vi.fn(),
  remove: vi.fn(),
  refresh: vi.fn(),
  loadMore: vi.fn(),
  acknowledge: vi.fn(),
  showToast: vi.fn(),
  isCreating: false,
  pendingById: {} as Record<string, boolean>,
  tasks: [] as ReturnType<typeof makeAutomationTask>[],
  runs: [] as ReturnType<typeof makeAutomationRun>[]
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))

vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: service.showToast })
}))

vi.mock('../useAutomations', () => ({
  useAutomations: ({ filter = 'all', query = '' }: { filter?: string; query?: string }) => {
    const normalized = query.toLowerCase()
    const tasks = service.tasks.filter(
      (task) =>
        (filter === 'all' || task.status === filter) &&
        (!normalized || task.title.toLowerCase().includes(normalized))
    )
    return {
      tasks,
      counts: {
        all: service.tasks.length,
        active: service.tasks.filter((task) => task.status === 'active').length,
        paused: service.tasks.filter((task) => task.status === 'paused').length
      },
      attentionCount: 0,
      lastSequence: 1,
      nextCursor: null,
      status: 'ready',
      error: null,
      mutationError: null,
      isRefreshing: false,
      isLoadingMore: false,
      isCreating: service.isCreating,
      pendingById: service.pendingById,
      refresh: service.refresh,
      loadMore: service.loadMore,
      create: service.create,
      update: service.update,
      setEnabled: service.setEnabled,
      runNow: service.runNow,
      remove: service.remove
    }
  }
}))

vi.mock('../useAutomationDetail', () => ({
  useAutomationDetail: (automationId?: string | null) => ({
    task: service.tasks.find((task) => task.automationId === automationId) ?? null,
    status: 'ready',
    error: null,
    isRefreshing: false,
    refresh: service.refresh
  })
}))

vi.mock('../useAutomationRuns', () => ({
  useAutomationRuns: () => ({
    runs: service.runs,
    status: 'ready',
    error: null,
    nextCursor: null,
    isRefreshing: false,
    isLoadingMore: false,
    refresh: service.refresh,
    loadMore: service.loadMore
  })
}))

vi.mock('../useAutomationAttention', () => ({
  useAutomationAttention: () => ({
    items: [],
    unreadCount: 0,
    status: 'ready',
    error: null,
    nextCursor: null,
    isLoadingMore: false,
    refresh: service.refresh,
    loadMore: service.loadMore,
    acknowledge: service.acknowledge
  })
}))

const props = {
  conversations: [testConversation],
  defaultModelId: 'model-1',
  defaultPermissionMode: 'default' as const,
  defaultProjectId: 'project-1',
  models: [testModel],
  onClose: vi.fn(),
  onOpenConversation: vi.fn(),
  onOpenPermissionSettings: vi.fn(),
  permissionModeAvailability: { custom: true, full: true },
  projects: [{ id: 'project-1', name: 'Project One', createdAt: 1 }]
}

describe('ScheduledPage', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    service.tasks = []
    service.runs = []
    service.isCreating = false
    service.pendingById = {}
    service.refresh.mockResolvedValue(undefined)
  })

  it('filters real task rows and keeps the list interactive beside the drawer', async () => {
    service.tasks = [
      makeAutomationTask(),
      makeAutomationTask({
        automationId: 'automation-2',
        title: 'Paused task',
        status: 'paused',
        nextRunAt: null
      })
    ]
    const screen = await render(<ScheduledPage {...props} />)

    await expect.element(screen.getByText('Daily brief')).toBeVisible()
    await expect.element(screen.getByText('Paused task')).toBeVisible()
    await screen.getByRole('tab', { name: /automation.filterPaused/ }).click()
    await expect.element(screen.getByText('Paused task')).toBeVisible()
    await expect.element(screen.getByText('Daily brief')).not.toBeInTheDocument()

    await screen.getByText('Paused task').click()
    await expect.element(screen.getByRole('complementary')).toBeVisible()
    await expect.element(screen.getByRole('heading', { name: 'Paused task' })).toBeVisible()
    expect(document.querySelector('.app-confirm-dialog__backdrop')).toBeNull()
  })

  it('confirms before discarding a dirty create draft', async () => {
    service.tasks = [makeAutomationTask()]
    const screen = await render(<ScheduledPage {...props} />)
    await screen.getByRole('button', { name: 'automation.create', exact: true }).click()
    await screen.getByPlaceholder('automation.taskNamePlaceholder').fill('Unsaved title')
    await screen.getByRole('button', { name: 'automation.close', exact: true }).click()
    await expect.element(screen.getByRole('dialog')).toBeVisible()
    await expect.element(screen.getByText('automation.unsavedTitle')).toBeVisible()
    await screen
      .getByRole('dialog')
      .getByRole('button', { name: 'automation.cancel', exact: true })
      .last()
      .click()
    await expect.element(screen.getByRole('complementary')).toBeVisible()
  })

  it('opens a native notification task intent and focuses the requested run', async () => {
    service.tasks = [makeAutomationTask()]
    service.runs = [makeAutomationRun()]
    const screen = await render(
      <ScheduledPage
        {...props}
        openRequest={{ automationId: 'automation-1', runId: 'run-1', requestKey: 1 }}
      />
    )

    const focusedRun = document.querySelector<HTMLElement>('[data-focus="true"]')
    expect(focusedRun).not.toBeNull()
    await expect.element(screen.getByText('Done')).toBeVisible()
  })

  it('routes external chat navigation through the same dirty confirmation', async () => {
    service.tasks = [makeAutomationTask()]
    const proceed = vi.fn()
    const screen = await render(<ScheduledPage {...props} />)
    await screen.getByRole('button', { name: 'automation.create', exact: true }).click()
    await screen.getByPlaceholder('automation.taskNamePlaceholder').fill('Unsaved title')

    await screen.rerender(
      <ScheduledPage {...props} externalNavigationRequest={{ proceed, requestKey: 1 }} />
    )
    await expect.element(screen.getByText('automation.unsavedTitle')).toBeVisible()
    expect(proceed).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'automation.discard' }).click()
    expect(proceed).toHaveBeenCalledOnce()
  })

  it('keeps a conflicted user edit in place while refreshing the authoritative task', async () => {
    service.tasks = [makeAutomationTask()]
    service.update.mockRejectedValueOnce(
      Object.assign(new Error('server conflict'), { code: 'revision_conflict' })
    )
    const screen = await render(<ScheduledPage {...props} />)
    await screen.getByText('Daily brief').click()
    const title = screen.getByPlaceholder('automation.taskNamePlaceholder')
    await title.fill('My local edit')
    await screen.getByRole('button', { name: 'automation.save' }).click()

    await expect.element(title).toHaveValue('My local edit')
    await expect.element(screen.getByText('automation.revisionConflict')).toBeVisible()
    expect(service.refresh).toHaveBeenCalled()
  })

  it('disables drawer exit while a create request is in flight', async () => {
    let resolveCreate!: (task: ReturnType<typeof makeAutomationTask>) => void
    service.create.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveCreate = resolve
        })
    )
    const screen = await render(<ScheduledPage {...props} />)
    await screen.getByRole('button', { name: 'automation.create', exact: true }).click()
    await screen.getByPlaceholder('automation.taskNamePlaceholder').fill('New task')
    await screen.getByPlaceholder('automation.promptPlaceholder').fill('Do the work')
    await screen
      .getByRole('complementary')
      .getByRole('button', { name: 'automation.createTask' })
      .click()

    const close = screen.getByRole('button', { name: 'automation.close', exact: true })
    await expect.element(close).toBeDisabled()
    await close.click({ force: true })
    await expect.element(screen.getByRole('complementary')).toBeVisible()

    const created = makeAutomationTask({ automationId: 'automation-created', title: 'New task' })
    service.tasks = [created]
    resolveCreate(created)
    await expect.element(screen.getByRole('heading', { name: 'New task' })).toBeVisible()
  })

  it('keeps the dedicated page close control available at narrow widths', async () => {
    await page.viewport(500, 720)
    try {
      const screen = await render(<ScheduledPage {...props} />)
      await expect
        .element(screen.getByRole('button', { name: 'automation.closePage' }))
        .toBeVisible()
    } finally {
      await page.viewport(1280, 720)
    }
  })

  it('makes the covered narrow list inert and restores focus when the drawer closes', async () => {
    service.tasks = [makeAutomationTask()]
    await page.viewport(600, 720)
    try {
      const screen = await render(<ScheduledPage {...props} />)
      const opener = document.querySelector<HTMLButtonElement>('.automation-task-row__main')!
      opener.focus()
      opener.click()

      await expect
        .poll(() => document.querySelector<HTMLElement>('.scheduled-page__list-pane')?.inert)
        .toBe(true)
      expect(
        document
          .querySelector<HTMLElement>('.scheduled-page__list-pane')
          ?.getAttribute('aria-hidden')
      ).toBe('true')

      await screen
        .getByRole('complementary')
        .getByRole('button', { name: 'automation.close' })
        .click()
      await expect.poll(() => document.activeElement).toBe(opener)
    } finally {
      await page.viewport(1280, 720)
    }
  })

  it('keeps authoritative row state after a failed pause and explains the failure', async () => {
    service.tasks = [makeAutomationTask()]
    service.setEnabled.mockRejectedValueOnce(new Error('offline'))
    const screen = await render(<ScheduledPage {...props} />)

    await screen.getByRole('button', { name: /automation.moreActions/ }).click()
    await screen.getByRole('menuitem', { name: 'automation.pause' }).click()
    await vi.waitFor(() => {
      expect(service.showToast).toHaveBeenCalledWith('automation.pauseFailed', {
        durationMs: 5000
      })
    })
    await expect.element(screen.getByText('Daily brief')).toBeVisible()

    await screen.getByRole('button', { name: /automation.moreActions/ }).click()
    await expect.element(screen.getByRole('menuitem', { name: 'automation.pause' })).toBeVisible()
  })

  it('distinguishes a search miss from an empty automation account', async () => {
    service.tasks = [makeAutomationTask()]
    const screen = await render(<ScheduledPage {...props} />)
    await screen.getByRole('searchbox').fill('does not exist')

    await expect.element(screen.getByText('automation.noResultsTitle')).toBeVisible()
    await expect.element(screen.getByText('automation.emptyTitle')).not.toBeInTheDocument()
    await screen.getByRole('button', { name: 'automation.clearSearch' }).last().click()
    await expect.element(screen.getByText('Daily brief')).toBeVisible()
  })

  it('renders non-empty large/light and narrow/dark visual captures without horizontal overflow', async () => {
    service.tasks = [makeAutomationTask()]
    const visualStyle = (dark: boolean) =>
      ({
        ...getFrontendCssVariables(frontendConfig, dark ? classicDarkTheme : classicLightTheme),
        '--titlebar-height': '36px',
        background: 'var(--mc-color-surface-main-panel)',
        color: 'var(--mc-color-text-primary)',
        height: '100vh',
        width: '100vw'
      }) as CSSProperties
    await page.viewport(1400, 800)
    try {
      const screen = await render(
        <div data-testid="automation-visual" style={visualStyle(false)}>
          <ScheduledPage
            {...props}
            openRequest={{ automationId: 'automation-1', runId: null, requestKey: 30 }}
          />
        </div>
      )
      const visual = screen.getByTestId('automation-visual').element() as HTMLElement
      const pageElement = visual.querySelector<HTMLElement>('.scheduled-page')!
      const drawer = visual.querySelector<HTMLElement>('.automation-drawer')!
      const lightCapture = await page.screenshot({ element: visual, save: false })
      expect(lightCapture.length).toBeGreaterThan(100)
      expect(pageElement.scrollWidth).toBeLessThanOrEqual(pageElement.clientWidth + 1)
      expect(drawer.getBoundingClientRect().width).toBeLessThan(
        pageElement.getBoundingClientRect().width
      )
      const lightBackground = getComputedStyle(pageElement).backgroundColor

      await page.viewport(600, 760)
      await screen.rerender(
        <div data-testid="automation-visual" style={visualStyle(true)}>
          <ScheduledPage
            {...props}
            openRequest={{ automationId: 'automation-1', runId: null, requestKey: 31 }}
          />
        </div>
      )
      const narrowVisual = screen.getByTestId('automation-visual').element() as HTMLElement
      const narrowPage = narrowVisual.querySelector<HTMLElement>('.scheduled-page')!
      const narrowDrawer = narrowVisual.querySelector<HTMLElement>('.automation-drawer')!
      const darkCapture = await page.screenshot({ element: narrowVisual, save: false })
      expect(darkCapture.length).toBeGreaterThan(100)
      expect(narrowPage.scrollWidth).toBeLessThanOrEqual(narrowPage.clientWidth + 1)
      expect(
        Math.abs(narrowDrawer.getBoundingClientRect().width - narrowPage.clientWidth)
      ).toBeLessThanOrEqual(1)
      expect(getComputedStyle(narrowPage).backgroundColor).not.toBe(lightBackground)
      await expect
        .element(
          screen.getByRole('complementary').getByRole('button', { name: 'automation.close' })
        )
        .toBeVisible()
    } finally {
      await page.viewport(1280, 720)
    }
  })
})
