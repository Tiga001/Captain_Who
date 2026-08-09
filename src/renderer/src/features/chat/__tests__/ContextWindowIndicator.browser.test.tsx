import type { AgentContextWindowSnapshot } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ContextWindowIndicator } from '../components/ContextWindowIndicator'

const translations: Record<string, string> = {
  'chat.contextWindow': '上下文窗口',
  'chat.contextUsedSummary': '{used}% 已用（剩余 {remaining}%）',
  'chat.contextTokenSummary': '已用 {used}，共 {total}',
  'chat.contextWindowAria': '上下文窗口已用 {used}%'
}

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => translations[key] ?? key
  })
}))

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
      goalTokens: 0,
      todoTokens: 0,
      providerContinuationTokens: 0,
      recentHistoryTokens: started ? inputTokens - 19_000 : 0,
      totalInputTokens: inputTokens
    },
    remainingInputTokens: 213_200 - inputTokens
  }
}

describe('ContextWindowIndicator', () => {
  it('shows zero before the first model context is created', async () => {
    const screen = await render(<ContextWindowIndicator snapshot={snapshot(0)} />)

    await expect.element(screen.getByText('0% 已用（剩余 100%）')).toBeVisible()
    await expect.element(screen.getByText('已用 0，共 213.2K')).toBeVisible()
    await expect.element(screen.getByRole('button', { name: '上下文窗口已用 0%' })).toBeVisible()
  })

  it('renders the complete request after conversation start', async () => {
    const screen = await render(<ContextWindowIndicator snapshot={snapshot(106_600)} />)

    await expect.element(screen.getByText('50% 已用（剩余 50%）')).toBeVisible()
    await expect.element(screen.getByText('已用 106.6K，共 213.2K')).toBeVisible()
  })

  it('enters the compaction-critical state at the same ninety-percent ratio', async () => {
    const screen = await render(<ContextWindowIndicator snapshot={snapshot(191_880)} />)

    await expect.element(screen.getByText('90% 已用（剩余 10%）')).toBeVisible()
    expect(screen.container.querySelector('.composer-context-window')).toHaveAttribute(
      'data-level',
      'critical'
    )
  })

  it('does not round a request below the compaction boundary up to ninety percent', async () => {
    const screen = await render(<ContextWindowIndicator snapshot={snapshot(191_879)} />)

    await expect.element(screen.getByText('89% 已用（剩余 11%）')).toBeVisible()
    expect(screen.container.querySelector('.composer-context-window')).toHaveAttribute(
      'data-level',
      'warning'
    )
  })
})
