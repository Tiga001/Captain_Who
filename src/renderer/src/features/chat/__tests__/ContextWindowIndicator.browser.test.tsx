import type { AgentContextWindowSnapshot } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ContextWindowIndicator } from '../components/ContextWindowIndicator'

const settings = vi.hoisted(() => ({
  language: 'zh-CN' as 'zh-CN' | 'en-US'
}))

vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/languageRegistry')
  return {
    useFrontendConfig: () => ({
      language: settings.language,
      t: (key: Parameters<typeof getTranslation>[1]) => getTranslation(settings.language, key)
    })
  }
})

function snapshot(inputTokens: number): AgentContextWindowSnapshot {
  const started = inputTokens > 0
  return {
    model: 'gpt-test',
    status: 'within_budget',
    contextWindowTokens: 256_000,
    reservedOutputTokens: 30_000,
    safetyMarginTokens: 12_800,
    inputCapacityTokens: 213_200,
    inputTokens,
    costBreakdown: {
      systemTokens: started ? 12_000 : 0,
      toolSchemaTokens: started ? 7_000 : 0,
      summaryTokens: 0,
      worldStateTokens: 0,
      todoTokens: 0,
      providerContinuationTokens: 0,
      recentHistoryTokens: started ? inputTokens - 19_000 : 0,
      totalInputTokens: inputTokens
    },
    remainingInputTokens: 213_200 - inputTokens
  }
}

describe('ContextWindowIndicator', () => {
  beforeEach(() => {
    settings.language = 'zh-CN'
  })

  it('shows zero before the first model context is created', async () => {
    const screen = await render(<ContextWindowIndicator snapshot={snapshot(0)} />)

    await expect.element(screen.getByText('估算已用 0%（剩余 100%）')).toBeVisible()
    await expect.element(screen.getByText('估算用量 0，容量 213.2K')).toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: '上下文窗口估算已用 0%' }))
      .toBeVisible()
  })

  it('renders the complete request after conversation start', async () => {
    const screen = await render(<ContextWindowIndicator snapshot={snapshot(106_600)} />)

    await expect.element(screen.getByText('估算已用 50%（剩余 50%）')).toBeVisible()
    await expect.element(screen.getByText('估算用量 106.6K，容量 213.2K')).toBeVisible()
  })

  it('labels the usage estimate in the English tooltip and accessible name', async () => {
    settings.language = 'en-US'
    const screen = await render(<ContextWindowIndicator snapshot={snapshot(106_600)} />)

    await expect.element(screen.getByText('Estimated 50% used (50% remaining)')).toBeVisible()
    await expect.element(screen.getByText('Estimated use: 106.6K of 213.2K')).toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: 'Context window estimated 50% used' }))
      .toBeVisible()
  })

  it('shows critical capacity pressure at ninety percent', async () => {
    const screen = await render(<ContextWindowIndicator snapshot={snapshot(191_880)} />)

    await expect.element(screen.getByText('估算已用 90%（剩余 10%）')).toBeVisible()
    expect(screen.container.querySelector('.composer-context-window')).toHaveAttribute(
      'data-level',
      'critical'
    )
  })

  it('does not round estimated usage below ninety percent up to critical pressure', async () => {
    const screen = await render(<ContextWindowIndicator snapshot={snapshot(191_879)} />)

    await expect.element(screen.getByText('估算已用 89%（剩余 11%）')).toBeVisible()
    expect(screen.container.querySelector('.composer-context-window')).toHaveAttribute(
      'data-level',
      'warning'
    )
  })
})
