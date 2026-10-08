import { act, StrictMode, useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { AgentUsageDashboardInput, AgentUsageDashboardOutput } from '@mycopilot/protocol'
import {
  usageDashboardCache,
  USAGE_DASHBOARD_FRESH_MS
} from '../../features/agent/usageDashboardCache'
import type { UiPreferencesSnapshot } from '../../features/storage/storageClient'

const agentClient = vi.hoisted(() => ({
  clearUsageRecords: vi.fn(),
  getUsageSummary: vi.fn(),
  getUsageDashboard: vi.fn()
}))
const frontendConfig = vi.hoisted(() => ({
  language: 'en-US',
  t: (key: string) => key,
  showCacheHitRate: false,
  setShowCacheHitRate: vi.fn()
}))
const modelSettings = vi.hoisted(() => ({
  models: [] as Array<{
    id: string
    displayName: string
  }>
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: function useFrontendConfigMock() {
    const [showCacheHitRate, setShowCacheHitRate] = useState(frontendConfig.showCacheHitRate)
    return {
      ...frontendConfig,
      showCacheHitRate,
      setShowCacheHitRate: (value: boolean) => {
        frontendConfig.setShowCacheHitRate(value)
        setShowCacheHitRate(value)
      }
    }
  }
}))

vi.mock('../../features/agent/agentClient', () => ({
  clearAgentUsageRecords: async () => {
    const finish = usageDashboardCache.beginClear()
    try {
      return await agentClient.clearUsageRecords()
    } finally {
      finish()
    }
  },
  getAgentUsageDashboard: agentClient.getUsageDashboard
}))

vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({ models: modelSettings.models })
}))

const { UsageBillingSettingsPage } =
  await import('../../features/settings/pages/UsageBillingSettingsPage')

const uiPreferences: UiPreferencesSnapshot = {
  profileAvatarDataUrl: null,
  profileDisplayName: '',
  profileHandle: 'USER',
  sidebarConversationSort: 'updated',
  sidebarProjectSort: 'created',
  sidebarProjectOrder: [],
  sidebarSectionOrder: 'projects_first',
  nativeFontSmoothing: false,
  showTokenUsageDetails: true,
  showContextWindowUsage: true,
  translucentSidebar: false,
  translucentSidebarTransparency: 54,
  fullPermissionEnabled: true,
  customPermissionEnabled: true,
  customPermissions: {
    read: 'workspace_only',
    write: 'workspace_only',
    command: 'require_approval',
    commandSafety: 'guarded',
    patch: 'require_approval',
    builtinExecution: 'require_approval'
  },
  updatedAt: 0
}

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
function dashboardFor(input: AgentUsageDashboardInput, count: number): AgentUsageDashboardOutput {
  const summary = {
    requestCount: count,
    messageCount: count,
    unpricedMessageCount: 0,
    inputTokens: count * 100,
    cachedInputTokens: count * 20,
    outputTokens: count * 10,
    models: []
  }
  return { summary, buckets: input.windows.map(() => summary) }
}
const billingPage = () => (
  <UsageBillingSettingsPage onUiPreferencesChange={vi.fn()} uiPreferences={uiPreferences} />
)
const requestCount = () => document.querySelector('.usage-summary-card strong')?.textContent
const advance = async (ms: number) => {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

describe('UsageBillingSettingsPage cached input', () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
    vi.setSystemTime(new Date(2026, 9, 8, 12))
    agentClient.clearUsageRecords.mockReset()
    agentClient.getUsageSummary.mockReset()
    agentClient.getUsageDashboard.mockReset()
    agentClient.getUsageDashboard.mockImplementation(
      async ({ windows }: AgentUsageDashboardInput) => ({
        summary: await agentClient.getUsageSummary({
          from: windows[0].from,
          to: windows.at(-1)!.to
        }),
        buckets: await Promise.all(windows.map((window) => agentClient.getUsageSummary(window)))
      })
    )
    usageDashboardCache.invalidate()
    frontendConfig.language = 'en-US'
    modelSettings.models = []
    frontendConfig.showCacheHitRate = false
    frontendConfig.setShowCacheHitRate.mockReset()
  })

  it('charts cached input separately without counting it again as uncached input', async () => {
    agentClient.getUsageSummary.mockResolvedValue({
      requestCount: 1,
      messageCount: 1,
      unpricedMessageCount: 0,
      inputTokens: 1_000,
      cachedInputTokens: 400,
      outputTokens: 100,
      outputThinkingTokens: 20,
      totalTokens: 1_100,
      estimatedCost: 0.01,
      models: []
    })

    await render(
      <UsageBillingSettingsPage onUiPreferencesChange={vi.fn()} uiPreferences={uiPreferences} />
    )

    await expect.poll(() => document.querySelectorAll('.usage-chart-day').length).toBe(7)

    const dayLabels = Array.from(document.querySelectorAll<HTMLElement>('.usage-chart-day')).map(
      (day) => day.getAttribute('aria-label') ?? ''
    )
    expect(
      dayLabels.every(
        (label) =>
          label.includes('usageBilling.uncachedInputTokens 600') &&
          label.includes('usageBilling.cachedInputTokens 400')
      )
    ).toBe(true)
    expect(
      document.querySelectorAll('.usage-chart-day span.usage-chart-bar--cached-input')
    ).toHaveLength(7)
  })

  it('uses visible model identity and never renders the internal configuration UUID', async () => {
    const internalId = '0197f53a-24e8-7a61-b630-secret-config-id'
    const deletedInternalId = '0197f53a-24e8-7a61-b630-deleted-config-id'
    modelSettings.models = [
      {
        id: internalId,
        displayName: 'DeepSeek'
      }
    ]
    agentClient.getUsageSummary.mockResolvedValue({
      requestCount: 2,
      messageCount: 2,
      unpricedMessageCount: 0,
      inputTokens: 1_000,
      cachedInputTokens: 0,
      outputTokens: 100,
      outputThinkingTokens: 20,
      totalTokens: 1_100,
      estimatedCost: 0.01,
      models: [
        {
          modelId: internalId,
          modelName: 'Historical DeepSeek Name',
          isConfigured: true,
          requestCount: 1,
          messageCount: 1,
          unpricedMessageCount: 0,
          inputTokens: 500,
          outputTokens: 50,
          outputThinkingTokens: 10
        },
        {
          modelId: deletedInternalId,
          modelName: 'Removed model',
          isConfigured: false,
          requestCount: 1,
          messageCount: 1,
          unpricedMessageCount: 0,
          inputTokens: 500,
          outputTokens: 50,
          outputThinkingTokens: 10
        }
      ]
    })

    const screen = await render(
      <UsageBillingSettingsPage onUiPreferencesChange={vi.fn()} uiPreferences={uiPreferences} />
    )

    await expect.element(screen.getByText('DeepSeek', { exact: true })).toBeVisible()
    await expect.element(screen.getByText('Removed model', { exact: true })).toBeVisible()
    expect(document.body.textContent).not.toContain(internalId)
    expect(document.body.textContent).not.toContain(deletedInternalId)
  })

  it('toggles the summary between token count and two-decimal percentage without changing charts', async () => {
    agentClient.getUsageSummary.mockResolvedValue({
      requestCount: 1,
      messageCount: 1,
      unpricedMessageCount: 0,
      inputTokens: 10_000,
      cachedInputTokens: 9_011,
      outputTokens: 1_000,
      totalTokens: 11_000,
      estimatedCost: 0.1,
      models: []
    })
    const onPreferencesChange = vi.fn()
    const screen = await render(
      <UsageBillingSettingsPage
        uiPreferences={{ ...uiPreferences, showTokenUsageDetails: false }}
        onUiPreferencesChange={onPreferencesChange}
      />
    )
    const cachedSummary = (): string | null | undefined =>
      document.querySelector('.usage-secondary-stat')?.textContent

    await expect.poll(cachedSummary).toBe('usageBilling.cachedInputTokens9,011')
    const chartLabels = Array.from(document.querySelectorAll('.usage-chart-day')).map((day) =>
      day.getAttribute('aria-label')
    )
    const summaryText = document.querySelector('.usage-summary-grid')?.textContent
    const switches = screen.getByRole('switch')
    await expect.element(switches.nth(0)).toHaveAccessibleName('usageBilling.tokenDetails')
    await expect.element(switches.nth(1)).toHaveAccessibleName('usageBilling.showCacheHitRate')

    await screen.getByRole('switch', { name: 'usageBilling.showCacheHitRate' }).click()
    await expect.poll(cachedSummary).toBe('usageBilling.cacheHitRate90.11%')
    expect(frontendConfig.setShowCacheHitRate).toHaveBeenLastCalledWith(true)
    expect(onPreferencesChange).not.toHaveBeenCalled()
    await expect
      .element(screen.getByRole('switch', { name: 'usageBilling.tokenDetails' }))
      .toHaveAttribute('aria-checked', 'false')
    expect(document.querySelector('.usage-summary-grid')?.textContent).toBe(summaryText)
    expect(
      Array.from(document.querySelectorAll('.usage-chart-day')).map((day) =>
        day.getAttribute('aria-label')
      )
    ).toEqual(chartLabels)

    await screen.getByRole('switch', { name: 'usageBilling.showCacheHitRate' }).click()
    await expect.poll(cachedSummary).toBe('usageBilling.cachedInputTokens9,011')
    expect(frontendConfig.setShowCacheHitRate).toHaveBeenLastCalledWith(false)
    expect(onPreferencesChange).not.toHaveBeenCalled()
  })

  it('uses aggregate input tokens and follows the selected model and time range', async () => {
    const modelA = {
      modelId: 'model-a',
      modelName: 'Model A',
      isConfigured: true,
      requestCount: 1,
      messageCount: 1,
      unpricedMessageCount: 0,
      inputTokens: 1_000,
      cachedInputTokens: 400,
      outputTokens: 100,
      totalTokens: 1_100
    }
    agentClient.getUsageSummary.mockImplementation(({ from, to }) => {
      const isYearSummary = to - from > 200 * 24 * 60 * 60 * 1_000
      return Promise.resolve({
        requestCount: 2,
        messageCount: 2,
        unpricedMessageCount: 0,
        inputTokens: isYearSummary ? 2_000 : 1_100,
        cachedInputTokens: isYearSummary ? 1_500 : 401,
        outputTokens: 150,
        totalTokens: isYearSummary ? 2_150 : 1_250,
        estimatedCost: 0.1,
        models: [
          { ...modelA, cachedInputTokens: isYearSummary ? 800 : 400 },
          {
            ...modelA,
            modelId: 'model-b',
            modelName: 'Model B',
            inputTokens: isYearSummary ? 1_000 : 100,
            cachedInputTokens: isYearSummary ? 700 : 1,
            outputTokens: 50,
            totalTokens: isYearSummary ? 1_050 : 150
          }
        ]
      })
    })
    frontendConfig.showCacheHitRate = true
    const screen = await render(
      <UsageBillingSettingsPage onUiPreferencesChange={vi.fn()} uiPreferences={uiPreferences} />
    )
    const cachedSummary = (): string | null | undefined =>
      document.querySelector('.usage-secondary-stat')?.textContent

    await expect.poll(cachedSummary).toBe('usageBilling.cacheHitRate36.45%')
    await screen.getByRole('button', { name: 'usageBilling.allModels' }).click()
    await screen.getByRole('option', { name: 'Model A' }).click()
    await expect.poll(cachedSummary).toBe('usageBilling.cacheHitRate40.00%')
    await screen.getByRole('button', { name: 'usageBilling.rangeLastYear' }).click()
    await expect.poll(cachedSummary).toBe('usageBilling.cacheHitRate80.00%')
    await screen.getByRole('button', { name: 'Model A' }).click()
    await screen.getByRole('option', { name: 'usageBilling.allModels' }).click()
    await expect.poll(cachedSummary).toBe('usageBilling.cacheHitRate75.00%')
  })

  it('shows an unavailable percentage when the input token count is zero', async () => {
    agentClient.getUsageSummary.mockResolvedValue({
      requestCount: 0,
      messageCount: 0,
      unpricedMessageCount: 0,
      inputTokens: 0,
      cachedInputTokens: 0,
      outputTokens: 0,
      totalTokens: 0,
      estimatedCost: 0,
      models: []
    })
    frontendConfig.showCacheHitRate = true
    const screen = await render(
      <UsageBillingSettingsPage onUiPreferencesChange={vi.fn()} uiPreferences={uiPreferences} />
    )
    await expect.element(screen.getByText('usageBilling.empty', { exact: true })).toBeVisible()
    expect(document.querySelector('.usage-secondary-stat')?.textContent).toBe(
      'usageBilling.cacheHitRate—'
    )
  })

  it('loads each range with one batch and reuses pending and fresh reads across StrictMode remounts', async () => {
    const pending = deferred<AgentUsageDashboardOutput>()
    agentClient.getUsageDashboard.mockReturnValue(pending.promise)
    const first = await render(<StrictMode>{billingPage()}</StrictMode>)
    await expect.poll(() => agentClient.getUsageDashboard.mock.calls.length).toBe(1)
    await first.unmount()
    const second = await render(<StrictMode>{billingPage()}</StrictMode>)
    expect(agentClient.getUsageDashboard).toHaveBeenCalledTimes(1)
    await act(async () =>
      pending.resolve(dashboardFor(agentClient.getUsageDashboard.mock.calls[0][0], 7))
    )
    await expect.poll(requestCount).toBe('7')
    await second.unmount()
    const third = await render(billingPage())
    expect(document.querySelector('.usage-chart-empty')).toBeNull()
    expect(document.querySelectorAll('.usage-chart-day')).toHaveLength(7)
    expect(agentClient.getUsageDashboard).toHaveBeenCalledTimes(1)
    frontendConfig.language = 'fr-FR'
    await third.rerender(billingPage())
    expect(agentClient.getUsageDashboard).toHaveBeenCalledTimes(1)

    agentClient.getUsageDashboard.mockImplementation(async (input: AgentUsageDashboardInput) =>
      dashboardFor(input, input.windows.length)
    )
    await third.getByRole('button', { name: 'usageBilling.rangeLast30Days' }).click()
    await expect.poll(requestCount).toBe('30')
    expect(agentClient.getUsageDashboard).toHaveBeenCalledTimes(2)
    expect(agentClient.getUsageDashboard.mock.calls[1][0].windows).toHaveLength(30)
    await third.getByRole('button', { name: 'usageBilling.rangeLastYear' }).click()
    await expect.poll(requestCount).toBe('12')
    expect(agentClient.getUsageDashboard).toHaveBeenCalledTimes(3)
    expect(agentClient.getUsageDashboard.mock.calls[2][0].windows).toHaveLength(12)
  })

  it('shows stale cached charts immediately and keeps their nodes and scroll when refresh fails', async () => {
    agentClient.getUsageDashboard.mockImplementation(async (input: AgentUsageDashboardInput) =>
      dashboardFor(input, 1)
    )
    const first = await render(billingPage())
    await expect.poll(requestCount).toBe('1')
    await first.unmount()
    await advance(USAGE_DASHBOARD_FRESH_MS)
    const pending = deferred<AgentUsageDashboardOutput>()
    agentClient.getUsageDashboard.mockReturnValue(pending.promise)
    await render(billingPage())
    expect(document.querySelectorAll('.usage-chart-day')).toHaveLength(7)
    expect(document.querySelector('.usage-chart-empty')).toBeNull()
    const scroller = document.querySelector<HTMLElement>('.usage-chart-scroller')!
    scroller.style.width = '100px'
    scroller.scrollLeft = 40
    const savedScroll = scroller.scrollLeft
    expect(savedScroll).toBeGreaterThan(0)
    await expect.poll(() => agentClient.getUsageDashboard.mock.calls.length).toBe(2)
    await act(async () => pending.reject(new Error('temporary refresh failure')))
    await expect
      .poll(() => document.querySelector('.usage-error-message')?.textContent)
      .toBeTruthy()
    expect(document.querySelector('.usage-chart-scroller')).toBe(scroller)
    expect(scroller.scrollLeft).toBe(savedScroll)
    expect(requestCount()).toBe('1')
    expect(document.querySelector('.usage-chart-empty')).toBeNull()
  })

  it.each(['success', 'error'])(
    'ignores an obsolete range %s without replacing the new range',
    async (outcome) => {
      const old = deferred<AgentUsageDashboardOutput>()
      agentClient.getUsageDashboard.mockImplementation((input: AgentUsageDashboardInput) =>
        input.windows.length === 7 ? old.promise : Promise.resolve(dashboardFor(input, 30))
      )
      const view = await render(billingPage())
      await expect.poll(() => agentClient.getUsageDashboard.mock.calls.length).toBe(1)
      await view.getByRole('button', { name: 'usageBilling.rangeLast30Days' }).click()
      await expect.poll(requestCount).toBe('30')
      await act(async () => {
        if (outcome === 'success')
          old.resolve(dashboardFor(agentClient.getUsageDashboard.mock.calls[0][0], 99))
        else old.reject(new Error('obsolete range failure'))
      })
      expect(requestCount()).toBe('30')
      expect(document.querySelectorAll('.usage-chart-day')).toHaveLength(30)
      expect(document.querySelector('.usage-error-message')).toBeNull()
    }
  )

  it('fences an in-flight response during clear and shares the empty result with later mounts', async () => {
    const old = deferred<AgentUsageDashboardOutput>()
    const clearing = deferred<{ deletedRecords: number }>()
    agentClient.getUsageDashboard
      .mockReturnValueOnce(old.promise)
      .mockImplementation(async (input: AgentUsageDashboardInput) => dashboardFor(input, 0))
    agentClient.clearUsageRecords.mockReturnValue(clearing.promise)
    const view = await render(billingPage())
    await expect.poll(() => agentClient.getUsageDashboard.mock.calls.length).toBe(1)
    await view.getByRole('button', { name: 'usageBilling.clear', exact: true }).click()
    await view.getByRole('button', { name: 'usageBilling.clearConfirm' }).click()
    expect(usageDashboardCache.isClearing).toBe(true)
    await act(async () =>
      old.resolve(dashboardFor(agentClient.getUsageDashboard.mock.calls[0][0], 99))
    )
    expect(requestCount()).not.toBe('99')
    expect(agentClient.getUsageDashboard).toHaveBeenCalledTimes(1)
    await act(async () => clearing.resolve({ deletedRecords: 99 }))
    await expect.poll(requestCount).toBe('0')
    expect(agentClient.getUsageDashboard).toHaveBeenCalledTimes(2)
    await view.unmount()
    await render(billingPage())
    expect(requestCount()).toBe('0')
    expect(agentClient.getUsageDashboard).toHaveBeenCalledTimes(2)
  })

  it('refreshes only while visible, rebuilds calendar windows after midnight and stops on unmount', async () => {
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible')
    agentClient.getUsageDashboard.mockImplementation(async (input: AgentUsageDashboardInput) =>
      dashboardFor(input, 1)
    )
    const view = await render(billingPage())
    await expect.poll(requestCount).toBe('1')
    await advance(USAGE_DASHBOARD_FRESH_MS)
    await expect.poll(() => agentClient.getUsageDashboard.mock.calls.length).toBe(2)
    const firstFrom = agentClient.getUsageDashboard.mock.calls[0][0].windows[0].from
    await act(async () => {
      visibility.mockReturnValue('hidden')
      document.dispatchEvent(new Event('visibilitychange'))
    })
    await advance(24 * 60 * 60 * 1000)
    await act(async () => window.dispatchEvent(new Event('focus')))
    expect(agentClient.getUsageDashboard).toHaveBeenCalledTimes(2)
    await act(async () => {
      visibility.mockReturnValue('visible')
      document.dispatchEvent(new Event('visibilitychange'))
    })
    await expect.poll(() => agentClient.getUsageDashboard.mock.calls.length).toBe(3)
    expect(agentClient.getUsageDashboard.mock.calls[2][0].windows[0].from).toBeGreaterThan(
      firstFrom
    )
    await view.unmount()
    await advance(USAGE_DASHBOARD_FRESH_MS * 2)
    await act(async () => window.dispatchEvent(new Event('focus')))
    expect(agentClient.getUsageDashboard).toHaveBeenCalledTimes(3)
  })
})
