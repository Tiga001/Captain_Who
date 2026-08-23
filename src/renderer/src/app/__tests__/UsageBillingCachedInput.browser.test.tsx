import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { UiPreferencesSnapshot } from '../../features/storage/storageClient'

const agentClient = vi.hoisted(() => ({
  clearUsageRecords: vi.fn(),
  getUsageSummary: vi.fn()
}))
const frontendConfig = vi.hoisted(() => ({
  language: 'en-US',
  t: (key: string) => key
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => frontendConfig
}))

vi.mock('../../features/agent/agentClient', () => ({
  clearAgentUsageRecords: agentClient.clearUsageRecords,
  getAgentUsageSummary: agentClient.getUsageSummary
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
})
