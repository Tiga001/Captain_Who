import { expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type { CSSProperties } from 'react'
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
  rootAnchorMessageId: string | null = null
): CollaborationTimelineActivity {
  return {
    activityId,
    agentId,
    occurredAt,
    rootAnchorMessageId,
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
  empty.unmount()

  const narrowStyle = {
    '--mc-color-border-subtle': '#cbd5e1',
    '--mc-color-focus-ring': '#2563eb',
    '--mc-color-status-success-text': '#047857',
    '--mc-color-text-accent': '#2563eb',
    '--mc-color-text-danger': '#b91c1c',
    '--mc-color-text-muted': '#64748b',
    '--mc-color-text-primary': '#0f172a',
    '--mc-color-text-secondary': '#475569',
    '--mc-font-size-chat-activity': '13px',
    background: '#ffffff',
    padding: 12,
    width: 280
  } as CSSProperties
  const screen = await render(
    <div style={narrowStyle}>
      <CollaborationTimelineActivityList
        activities={[
          activity(
            'event-anchor-a',
            'agent-a',
            'Accessibility review',
            'waiting_approval',
            6,
            5_000,
            'root-assistant'
          ),
          activity(
            'event-anchor-b',
            'agent-b',
            'Security review',
            'waiting_approval',
            7,
            7_000,
            'root-assistant'
          )
        ]}
        onOpenAgent={vi.fn()}
      />
    </div>
  )

  expect(screen.container.querySelectorAll('.collaboration-timeline__activity')).toHaveLength(1)
  expect(screen.container.querySelectorAll('.collaboration-timeline__chip')).toHaveLength(2)
  expect(screen.getByText('Waiting for approval')).toBeVisible()
  expect(
    screen.getByRole('button', {
      name: 'View sub-agent Accessibility review: Waiting for approval'
    })
  ).toBeVisible()
  const surface = screen.container.firstElementChild as HTMLElement
  expect(surface.scrollWidth).toBeLessThanOrEqual(surface.clientWidth)
  expect(
    getComputedStyle(screen.container.querySelector('.collaboration-timeline__chip')!).borderRadius
  ).toBe('999px')
  await page.screenshot({
    element: screen.container,
    path: '__screenshots__/CollaborationTimelineActivity.browser.test.tsx/narrow-inline.png'
  })
})
