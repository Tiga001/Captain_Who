import { expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type { CSSProperties } from 'react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import {
  CollaborationTimelineActivityList,
  type CollaborationTimelineActivity
} from '../CollaborationTimelineActivity'

const translations: Record<string, string> = {
  'collaboration.activity.openAgentActivity': 'View sub-agent {name}: {status}',
  'collaboration.activity.status.completed': 'Completed',
  'collaboration.activity.status.failed': 'Failed',
  'collaboration.activity.status.interrupted': 'Interrupted',
  'collaboration.activity.status.started': 'Started working',
  'collaboration.activity.status.updated': 'Updated',
  'collaboration.activity.status.waitingApproval': 'Waiting for approval'
}

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'en-US',
    t: (key: string) => translations[key] ?? key
  })
}))

function activity(
  activityId: string,
  agentId: string,
  taskNameSnapshot: string,
  semantic: CollaborationTimelineActivity['semantic'],
  sequence: number,
  occurredAt: number,
  rootAnchorMessageId: string | null = null,
  rootTraceBoundarySequence: number | null = rootAnchorMessageId === null ? null : sequence
): CollaborationTimelineActivity {
  return {
    activityId,
    agentId,
    occurredAt,
    rootAnchorMessageId,
    rootTraceBoundarySequence,
    runId: null,
    semantic,
    sequence,
    taskNameSnapshot,
    turnId: null
  }
}

it('deduplicates replay, merges adjacent agents, retains updates, and exposes exact navigation', async () => {
  const onOpenAgent = vi.fn()
  const first = activity('event-start-a', 'agent-a', 'Researcher', 'started', 1, 1_000)
  const screen = await render(
    <CollaborationTimelineActivityList
      activities={[
        activity('event-terminal-a', 'agent-a', 'Researcher', 'completed', 4, 3_000),
        first,
        activity('event-start-b', 'agent-b', 'Reviewer', 'started', 2, 1_001),
        activity('event-update-a', 'agent-a', 'Researcher', 'updated', 3, 2_000),
        first,
        activity('event-followup-a', 'agent-a', 'Researcher', 'started', 5, 4_000),
        activity('event-failed-c', 'agent-c', 'Tester', 'failed', 6, 5_000),
        activity('event-interrupted-d', 'agent-d', 'Writer', 'interrupted', 7, 6_000)
      ]}
      onOpenAgent={onOpenAgent}
    />
  )

  const rows = screen.container.querySelectorAll('.collaboration-timeline__activity')
  expect(rows).toHaveLength(6)
  expect(rows[0]).toHaveAttribute('data-semantic', 'started')
  expect(rows[0]?.querySelectorAll('.collaboration-timeline__chip')).toHaveLength(2)
  expect(rows[1]).toHaveAttribute('data-semantic', 'updated')
  expect(rows[2]).toHaveAttribute('data-semantic', 'completed')
  expect(rows[3]).toHaveAttribute('data-semantic', 'started')
  expect(rows[4]).toHaveAttribute('data-semantic', 'failed')
  expect(rows[5]).toHaveAttribute('data-semantic', 'interrupted')
  expect(screen.getByText('Failed')).toBeVisible()
  expect(screen.getByText('Interrupted')).toBeVisible()
  expect(screen.container.querySelectorAll('[data-agent-id="agent-a"]')).toHaveLength(4)

  const reviewer = screen.getByRole('button', {
    name: 'View sub-agent Reviewer: Started working'
  })
  for (
    let attempt = 0;
    attempt < 4 && document.activeElement !== reviewer.element();
    attempt += 1
  ) {
    await userEvent.keyboard('{Tab}')
  }
  expect(document.activeElement).toBe(reviewer.element())
  await userEvent.keyboard('{Enter}')
  expect(onOpenAgent).toHaveBeenCalledWith('agent-b')
  expect(screen.container.querySelector('input, textarea')).toBeNull()
})

it('renders no wrapper for an empty root and stays compact at narrow width', async () => {
  const empty = await render(
    <CollaborationTimelineActivityList activities={[]} onOpenAgent={vi.fn()} />
  )
  expect(empty.container.querySelector('[data-testid="collaboration-timeline"]')).toBeNull()
  await empty.unmount()

  const narrowStyle = {
    ...getFrontendCssVariables(frontendConfig, classicLightTheme),
    background: 'var(--mc-color-surface-main-panel)',
    padding: 12,
    width: 280
  } as CSSProperties
  const screen = await render(
    <div style={narrowStyle}>
      <CollaborationTimelineActivityList
        activities={[
          activity('event-start-a', 'agent-a', 'QA', 'started', 1, 1_000, 'root-start'),
          activity('event-start-b', 'agent-b', 'Docs', 'started', 2, 1_001, 'root-start', 1),
          activity(
            'event-completed',
            'agent-c',
            'Long compatibility and accessibility review',
            'completed',
            3,
            2_000,
            'root-completed'
          ),
          activity(
            'event-approval',
            'agent-d',
            'Deploy',
            'waiting_approval',
            4,
            3_000,
            'root-approval'
          ),
          activity('event-failed', 'agent-e', 'Tests', 'failed', 5, 4_000, 'root-failed')
        ]}
        onOpenAgent={vi.fn()}
      />
    </div>
  )

  const rows = screen.container.querySelectorAll('.collaboration-timeline__activity')
  expect(rows).toHaveLength(4)
  expect(rows[0]?.querySelectorAll('.collaboration-timeline__chip')).toHaveLength(2)
  expect(screen.container.querySelectorAll('.collaboration-timeline__chip')).toHaveLength(5)
  expect(screen.getByText('Started working')).toBeVisible()
  expect(screen.getByText('Completed')).toBeVisible()
  expect(screen.getByText('Waiting for approval')).toBeVisible()
  expect(screen.getByText('Failed')).toBeVisible()
  expect(
    screen.getByRole('button', {
      name: 'View sub-agent QA: Started working'
    })
  ).toBeVisible()
  const firstChips = rows[0]?.querySelector('.collaboration-timeline__chips') as HTMLElement
  const firstStatus = rows[0]?.querySelector('.collaboration-timeline__status') as HTMLElement
  expect(firstChips.nextElementSibling).toBe(firstStatus)
  expect(getComputedStyle(firstChips).display).toBe('contents')
  expect(getComputedStyle(firstStatus).color).toBe('rgb(79, 86, 96)')
  expect(getComputedStyle(screen.getByText('Completed').element()).color).toBe('rgb(79, 86, 96)')
  expect(getComputedStyle(screen.getByText('Waiting for approval').element()).color).toBe(
    'rgb(63, 63, 70)'
  )
  expect(getComputedStyle(screen.getByText('Failed').element()).color).toBe('rgb(180, 35, 24)')
  const surface = screen.container.firstElementChild as HTMLElement
  expect(surface.scrollWidth).toBeLessThanOrEqual(surface.clientWidth)
  const qa = screen.getByRole('button', { name: 'View sub-agent QA: Started working' })
  expect(getComputedStyle(qa.element()).borderRadius).toBe('999px')
  expect(getComputedStyle(qa.element()).color).toBe('rgb(79, 86, 96)')
  expect(qa.element().getBoundingClientRect().height).toBeGreaterThanOrEqual(24)
  const restingBackground = getComputedStyle(qa.element()).backgroundColor
  await page.screenshot({
    element: screen.container,
    path: '__screenshots__/CollaborationTimelineActivity.browser.test.tsx/narrow-inline.png'
  })

  await userEvent.hover(qa)
  await expect
    .poll(() => getComputedStyle(qa.element()).backgroundColor)
    .not.toBe(restingBackground)
  await expect.poll(() => getComputedStyle(qa.element()).color).toBe('rgb(63, 63, 70)')
  await userEvent.unhover(qa)
  await userEvent.keyboard('{Tab}')
  expect(document.activeElement).toBe(qa.element())
  expect(getComputedStyle(qa.element()).outlineWidth).toBe('2px')
})

it('uses readable product-theme status colors in classic dark mode', async () => {
  const darkStyle = {
    ...getFrontendCssVariables(frontendConfig, classicDarkTheme),
    background: 'var(--mc-color-surface-main-panel)',
    padding: 12,
    width: 280
  } as CSSProperties
  const screen = await render(
    <div style={darkStyle}>
      <CollaborationTimelineActivityList
        activities={[
          activity('event-dark-start', 'agent-dark-a', 'Reviewer', 'started', 1, 1_000),
          activity('event-dark-approval', 'agent-dark-b', 'Approver', 'waiting_approval', 2, 2_000)
        ]}
        onOpenAgent={vi.fn()}
      />
    </div>
  )

  expect(getComputedStyle(screen.getByText('Started working').element()).color).toBe(
    'rgb(224, 224, 224)'
  )
  expect(getComputedStyle(screen.getByText('Waiting for approval').element()).color).toBe(
    'rgb(252, 252, 252)'
  )
})
