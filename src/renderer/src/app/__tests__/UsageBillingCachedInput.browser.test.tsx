import { useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { UiPreferencesSnapshot } from '../../features/storage/storageClient'

const agentClient = vi.hoisted(() => ({
  clearUsageRecords: vi.fn(),
  getUsageSummary: vi.fn()
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
  clearAgentUsageRecords: agentClient.clearUsageRecords,
  getAgentUsageSummary: agentClient.getUsageSummary
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

describe('UsageBillingSettingsPage cached input', () => {
  beforeEach(() => {
    agentClient.clearUsageRecords.mockReset()
    agentClient.getUsageSummary.mockReset()
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
})
