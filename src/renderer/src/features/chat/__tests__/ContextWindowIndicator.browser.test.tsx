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
  return {
    model: 'gpt-test',
    status: 'within_budget',
    contextWindowTokens: 256_000,
    reservedOutputTokens: 30_000,
    safetyMarginTokens: 12_800,
    inputCapacityTokens: 191_500,
    inputTokens,
    costBreakdown: {
      systemTokens: 12_000,
      toolSchemaTokens: 9_700,
      summaryTokens: 0,
      worldStateTokens: 0,
      goalTokens: 0,
      todoTokens: 0,
      recentHistoryTokens: inputTokens,
      totalInputTokens: inputTokens + 21_700
    },
    remainingInputTokens: 191_500 - inputTokens
  }
}

describe('ContextWindowIndicator', () => {
  it('starts an empty conversation at zero after the backend removes its request baseline', async () => {
    const screen = await render(<ContextWindowIndicator snapshot={snapshot(0)} />)

    await expect.element(screen.getByText('0% 已用（剩余 100%）')).toBeVisible()
    await expect.element(screen.getByText('已用 0，共 191.5K')).toBeVisible()
    await expect.element(screen.getByRole('button', { name: '上下文窗口已用 0%' })).toBeVisible()
  })

  it('renders current-loop growth against the net conversation capacity', async () => {
    const screen = await render(<ContextWindowIndicator snapshot={snapshot(95_750)} />)

    await expect.element(screen.getByText('50% 已用（剩余 50%）')).toBeVisible()
    await expect.element(screen.getByText('已用 95.8K，共 191.5K')).toBeVisible()
  })
})
