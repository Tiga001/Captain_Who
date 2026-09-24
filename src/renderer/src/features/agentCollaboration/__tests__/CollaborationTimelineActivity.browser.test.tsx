import { expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type { CSSProperties } from 'react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import {
  CollaborationTimelineActivityList,
  copyCollaborationTimelineSelection,
  type CollaborationTimelineActivity
} from '../CollaborationTimelineActivity'

const translations: Record<string, string> = {
  'collaboration.activity.agentNameSeparator': ', ',
  'collaboration.activity.copyAgentStatus': '{name}: {status}',
  'collaboration.activity.moreAgents': '{count} more',
  'collaboration.activity.moreAgentsLabel': '{count} more sub-agents: {agents}, {status}',
  'collaboration.activity.openAgentActivity': 'View sub-agent {name}: {status}',
  'collaboration.activity.statusListSeparator': '; ',
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
  anchorMessageId: string | null = null,
  traceBoundarySequence: number | null = anchorMessageId === null ? null : sequence
): CollaborationTimelineActivity {
  return {
    activityId,
    agentId,
    occurredAt,
    parentAgentId: 'root:root-conversation',
    parentConversationId: 'root-conversation',
    anchorMessageId,
    traceBoundarySequence,
    runId: null,
    semantic,
    sequence,
    taskNameSnapshot,
    turnId: null
  }
}

function copySelection(target: Element): string {
  const clipboardData = new DataTransfer()
  target.dispatchEvent(
    new ClipboardEvent('copy', {
      bubbles: true,
      cancelable: true,
      clipboardData
    })
  )
  return clipboardData.getData('text/plain')
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
  const repeatedAgentAvatars = screen.container.querySelectorAll<HTMLElement>(
    '[data-agent-id="agent-a"] .agent-avatar'
  )
  expect(repeatedAgentAvatars).toHaveLength(4)
  expect(
    new Set(Array.from(repeatedAgentAvatars, (avatar) => avatar.dataset.agentAvatarIndex)).size
  ).toBe(1)
  expect(
    new Set(
      Array.from(repeatedAgentAvatars, (avatar) =>
        avatar.querySelector<HTMLImageElement>('img')?.getAttribute('src')
      )
    ).size
  ).toBe(1)

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
          activity('event-start-c', 'agent-c', 'Research', 'started', 3, 1_002, 'root-start', 1),
          activity('event-start-d', 'agent-d', 'Review', 'started', 4, 1_003, 'root-start', 1),
          activity('event-start-e', 'agent-e', 'Security', 'started', 5, 1_004, 'root-start', 1),
          activity(
            'event-completed',
            'agent-f',
            'Long compatibility and accessibility review',
            'completed',
            6,
            2_000,
            'root-completed'
          ),
          activity(
            'event-approval',
            'agent-g',
            'Deploy',
            'waiting_approval',
            7,
            3_000,
            'root-approval'
          ),
          activity('event-failed', 'agent-h', 'Tests', 'failed', 8, 4_000, 'root-failed')
        ]}
        onOpenAgent={vi.fn()}
      />
    </div>
  )

  const rows = screen.container.querySelectorAll('.collaboration-timeline__activity')
  expect(rows).toHaveLength(4)
  expect(rows[0]?.querySelectorAll('.collaboration-timeline__chip')).toHaveLength(3)
  expect(screen.container.querySelectorAll('.collaboration-timeline__chip')).toHaveLength(6)
  expect(screen.getByText('2 more')).toBeVisible()
  expect(
    screen.getByRole('note', {
      name: '2 more sub-agents: Review, Security, Started working'
    })
  ).toBeVisible()
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
  const firstOverflow = rows[0]?.querySelector('.collaboration-timeline__overflow') as HTMLElement
  const firstStatus = rows[0]?.querySelector('.collaboration-timeline__status') as HTMLElement
  expect(firstChips.compareDocumentPosition(firstOverflow) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING
  )
  expect(
    firstOverflow.compareDocumentPosition(firstStatus) & Node.DOCUMENT_POSITION_FOLLOWING
  ).toBe(Node.DOCUMENT_POSITION_FOLLOWING)
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

it('keeps only the latest same-Agent snapshot and exposes unambiguous native copy text', async () => {
  const onOpenAgent = vi.fn()
  const screen = await render(
    <div onCopy={copyCollaborationTimelineSelection}>
      <p>Before activity</p>
      <CollaborationTimelineActivityList
        activities={[
          activity('old-a', 'agent-a', 'Alpha old', 'completed', 1, 1_000, 'root', 9),
          activity('agent-b', 'agent-b', 'Beta', 'completed', 2, 1_100, 'root', 9),
          activity('new-a', 'agent-a', 'Alpha', 'completed', 3, 1_200, 'root', 9),
          activity('agent-c', 'agent-c', 'Gamma', 'completed', 4, 1_300, 'root', 9),
          activity('agent-d', 'agent-d', 'Delta', 'completed', 5, 1_400, 'root', 9)
        ]}
        onOpenAgent={onOpenAgent}
      />
      <p>After activity</p>
    </div>
  )

  expect(screen.container.querySelectorAll('.collaboration-timeline__activity')).toHaveLength(1)
  expect(screen.container.querySelectorAll('.collaboration-timeline__chip')).toHaveLength(3)
  expect(screen.getByText('Alpha')).toBeVisible()
  expect(screen.container.textContent).not.toContain('Alpha old')
  const row = screen.container.querySelector<HTMLElement>('.collaboration-timeline__activity')
  const selection = window.getSelection()
  const range = document.createRange()
  const alphaButton = screen.getByRole('button', {
    name: 'View sub-agent Alpha: Completed'
  })
  range.selectNodeContents(alphaButton.element())
  selection?.removeAllRanges()
  selection?.addRange(range)
  expect(copySelection(alphaButton.element())).toBe(
    'Alpha: Completed; Beta: Completed; Gamma: Completed; Delta: Completed'
  )

  range.selectNodeContents(row!)
  selection?.removeAllRanges()
  selection?.addRange(range)
  expect(copySelection(row!)).toBe(
    'Alpha: Completed; Beta: Completed; Gamma: Completed; Delta: Completed'
  )

  const wrapper = screen.container.firstElementChild!
  range.selectNodeContents(wrapper)
  selection?.removeAllRanges()
  selection?.addRange(range)
  const wholeConversationCopy = copySelection(wrapper)
  expect(wholeConversationCopy).toContain('Before activity')
  expect(wholeConversationCopy).toContain(
    'Alpha: Completed; Beta: Completed; Gamma: Completed; Delta: Completed'
  )
  expect(wholeConversationCopy).toContain('After activity')
  selection?.removeAllRanges()

  await userEvent.click(screen.getByRole('button', { name: 'View sub-agent Alpha: Completed' }))
  expect(onOpenAgent).toHaveBeenCalledWith('agent-a')
  expect(screen.getByRole('note', { name: '1 more sub-agents: Delta, Completed' })).toBeVisible()
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
