import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { CSSProperties } from 'react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicDarkTheme } from '../../../config/themes/classic'
import { AutomationTaskRow } from '../components/AutomationTaskRow'
import { makeAutomationRun, makeAutomationTask } from './automationUiFixtures'
import '../ScheduledPage.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))

describe('AutomationTaskRow', () => {
  it('opens the correct active menu without triggering the row', async () => {
    const onOpen = vi.fn()
    const onSetEnabled = vi.fn()
    const screen = await render(
      <AutomationTaskRow
        onDelete={vi.fn()}
        onOpen={onOpen}
        onRunNow={vi.fn()}
        onSetEnabled={onSetEnabled}
        task={makeAutomationTask()}
      />
    )

    await screen.getByRole('button', { name: /automation.moreActions/ }).click()
    expect(onOpen).not.toHaveBeenCalled()
    await expect.element(screen.getByRole('menuitem', { name: 'automation.pause' })).toBeVisible()
    await screen.getByRole('menuitem', { name: 'automation.pause' }).click()
    expect(onSetEnabled).toHaveBeenCalledWith(
      expect.objectContaining({ automationId: 'automation-1' }),
      false
    )
  })

  it('shows resume when paused and disables run now while a run is active', async () => {
    const screen = await render(
      <AutomationTaskRow
        onDelete={vi.fn()}
        onOpen={vi.fn()}
        onRunNow={vi.fn()}
        onSetEnabled={vi.fn()}
        task={makeAutomationTask({
          status: 'paused',
          latestRun: makeAutomationRun({ status: 'running', completedAt: null })
        })}
      />
    )

    await screen.getByRole('button', { name: /automation.moreActions/ }).click()
    await expect.element(screen.getByRole('menuitem', { name: 'automation.resume' })).toBeVisible()
    await expect
      .element(screen.getByRole('menuitem', { name: 'automation.runningAction' }))
      .toBeDisabled()
  })

  it('uses the shared product tokens in dark mode', async () => {
    const style = {
      ...getFrontendCssVariables(frontendConfig, classicDarkTheme),
      background: 'var(--mc-color-surface-main-panel)',
      width: 640
    } as CSSProperties
    const screen = await render(
      <div style={style}>
        <AutomationTaskRow
          onDelete={vi.fn()}
          onOpen={vi.fn()}
          onRunNow={vi.fn()}
          onSetEnabled={vi.fn()}
          task={makeAutomationTask()}
        />
      </div>
    )

    expect(getComputedStyle(screen.getByText('Daily brief').element()).color).toBe(
      'rgb(252, 252, 252)'
    )
  })

  it('highlights only unread attention', async () => {
    const attention = {
      schemaVersion: 1 as const,
      attentionId: 'attention-1',
      automationId: 'automation-1',
      runId: null,
      kind: 'configuration_blocked' as const,
      message: 'Repair needed',
      createdAt: 1_800_000_000_000,
      readAt: null
    }
    const screen = await render(
      <AutomationTaskRow
        onDelete={vi.fn()}
        onOpen={vi.fn()}
        onRunNow={vi.fn()}
        onSetEnabled={vi.fn()}
        task={makeAutomationTask({ attention })}
      />
    )

    await expect.element(screen.getByLabelText('automation.requiresAttention')).toBeVisible()
    await screen.rerender(
      <AutomationTaskRow
        onDelete={vi.fn()}
        onOpen={vi.fn()}
        onRunNow={vi.fn()}
        onSetEnabled={vi.fn()}
        task={makeAutomationTask({ attention: { ...attention, readAt: 1_800_000_000_100 } })}
      />
    )
    await expect
      .element(screen.getByLabelText('automation.requiresAttention'))
      .not.toBeInTheDocument()
  })
})
