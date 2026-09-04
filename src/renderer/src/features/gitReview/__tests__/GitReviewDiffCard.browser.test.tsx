import type { GitReviewFile } from '@mycopilot/protocol'
import { useRef, type CSSProperties, type ReactNode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { Tooltip } from '../../../components/overlay/Tooltip'
import { getFrontendTheme } from '../../../config/frontendTheme'
import { getGitReviewCssVariables } from '../../../config/themes/gitReviewTheme'
import type { Translate } from '../../../config/translationFormat'
import '../../../styles/global.css'
import { GitReviewDiffCard } from '../GitReviewDiffCard'
import '../GitReviewPanel.css'
import type { GitReviewDiffState, GitReviewFileContentState } from '../useGitReview'

const translate: Translate = (key) => key
const noop = (): void => undefined
const themeVariables = {
  ...getGitReviewCssVariables(getFrontendTheme('classic-dark').tokens.colors.gitReview),
  '--mc-color-border-default': 'rgb(75, 78, 84)',
  '--mc-color-border-hairline': 'rgb(66, 69, 74)',
  '--mc-color-border-subtle': 'rgb(61, 64, 69)',
  '--mc-color-diff-addition-text': 'rgb(127, 180, 104)',
  '--mc-color-diff-deletion-text': 'rgb(224, 108, 117)',
  '--mc-color-icon-default': 'rgb(225, 225, 225)',
  '--mc-color-icon-muted': 'rgb(155, 158, 164)',
  '--mc-color-state-hover': 'rgba(255, 255, 255, 0.08)',
  '--mc-color-surface-info-subtle': 'rgb(42, 50, 61)',
  '--mc-color-surface-muted': 'rgb(48, 51, 56)',
  '--mc-color-surface-popover': 'rgb(45, 48, 53)',
  '--mc-color-surface-right-panel': 'rgb(37, 40, 45)',
  '--mc-color-text-accent': 'rgb(125, 170, 220)',
  '--mc-color-text-muted': 'rgb(151, 154, 160)',
  '--mc-color-text-primary': 'rgb(232, 232, 232)',
  '--mc-color-text-secondary': 'rgb(190, 192, 197)',
  '--mc-font-size-base': '14px',
  '--mc-font-size-sm': '12px',
  '--mc-font-weight-medium': '500',
  '--mc-radius-sm': '6px',
  '--mc-shadow-composer': '0 8px 24px rgba(0, 0, 0, 0.3)'
} as const

beforeEach(() => {
  for (const [name, value] of Object.entries(themeVariables)) {
    document.documentElement.style.setProperty(name, value)
  }
})

afterEach(() => {
  for (const name of Object.keys(themeVariables)) {
    document.documentElement.style.removeProperty(name)
  }
})

function createFile(id: string): GitReviewFile {
  return {
    id,
    path: `src/${id}.ts`,
    stats: { additions: 1, deletions: 1 },
    status: 'modified'
  }
}

function createDiffState(fileId: string): GitReviewDiffState {
  const lineCount = 36
  const lines = Array.from(
    { length: lineCount },
    (_, index) => ` ${fileId} context line ${index + 1}`
  )

  return {
    status: 'ready',
    value: {
      fileId,
      patch: [`@@ -1,${lineCount} +1,${lineCount} @@`, ...lines].join('\n'),
      snapshotId: 'snapshot-1',
      status: 'ready'
    }
  }
}

function StickyCardsFixture(): ReactNode {
  const scrollRootRef = useRef<HTMLDivElement>(null)
  const style = {
    display: 'block',
    height: 'auto',
    width: '440px'
  } as CSSProperties

  return (
    <div className="git-review" style={style}>
      <div
        className="git-review__content"
        data-testid="review-scrollport"
        ref={scrollRootRef}
        style={{ height: '220px' }}
      >
        <div className="git-review__diff-list">
          {['first', 'second', 'third'].map((fileId) => (
            <GitReviewDiffCard
              diffState={createDiffState(fileId)}
              file={createFile(fileId)}
              isExpanded
              isNearViewport
              isReviewActive
              isSelected={fileId === 'first'}
              isVisible
              loadFullFiles={false}
              key={fileId}
              mutationLocked={false}
              mutationPending={false}
              onMutate={noop}
              onOpenFile={noop}
              onRequestDiff={noop}
              onRestore={noop}
              onToggle={noop}
              targetKind="unstaged"
              scrollRootRef={scrollRootRef}
              t={translate}
              viewMode="unified"
              wrapLines={false}
            />
          ))}
        </div>
      </div>
    </div>
  )
}

function WindowedCardFixture({
  active,
  layoutWidth = 440,
  near,
  viewMode = 'unified'
}: {
  active: boolean
  layoutWidth?: number
  near: boolean
  viewMode?: 'split' | 'unified'
}): ReactNode {
  const scrollRootRef = useRef<HTMLDivElement>(null)
  return (
    <div className="git-review" style={{ display: 'block', height: 420, width: 440 }}>
      <div className="git-review__content" ref={scrollRootRef} style={{ height: 420 }}>
        <GitReviewDiffCard
          diffState={createDiffState('windowed')}
          file={createFile('windowed')}
          isExpanded
          isNearViewport={near}
          isReviewActive={active}
          isSelected={false}
          isVisible={near}
          layoutWidth={layoutWidth}
          loadFullFiles={false}
          mutationLocked={false}
          mutationPending={false}
          onMutate={noop}
          onOpenFile={noop}
          onRequestDiff={noop}
          onRestore={noop}
          onToggle={noop}
          reviewSnapshotId="snapshot-1"
          targetKind="unstaged"
          scrollRootRef={scrollRootRef}
          t={translate}
          viewMode={viewMode}
          wrapLines={false}
        />
      </div>
    </div>
  )
}

function LastTurnCardFixture({
  onOpenFile = noop
}: {
  onOpenFile?: (path: string) => void
}): ReactNode {
  const scrollRootRef = useRef<HTMLDivElement>(null)
  return (
    <div className="git-review" style={{ display: 'block', height: 300, width: 440 }}>
      <div className="git-review__content" ref={scrollRootRef}>
        <GitReviewDiffCard
          diffState={createDiffState('last-turn')}
          file={createFile('last-turn')}
          isExpanded={false}
          isNearViewport
          isReviewActive
          isSelected
          isVisible
          loadFullFiles={false}
          mutationLocked={false}
          mutationPending={false}
          onMutate={noop}
          onOpenFile={onOpenFile}
          onRequestDiff={noop}
          onRestore={noop}
          onToggle={noop}
          targetKind="lastTurn"
          scrollRootRef={scrollRootRef}
          t={translate}
          viewMode="unified"
          wrapLines={false}
        />
      </div>
    </div>
  )
}

const ANCHOR_OLD_LINES = Array.from({ length: 80 }, (_, index) => `line ${index + 1}`)
const ANCHOR_NEW_LINES = ANCHOR_OLD_LINES.map((line, index) =>
  index === 49 ? `${line} changed` : line
)
const ANCHOR_PATCH = [
  '@@ -48,5 +48,5 @@',
  ' line 48',
  ' line 49',
  '-line 50',
  '+line 50 changed',
  ' line 51',
  ' line 52'
].join('\n')

function ExpansionAnchorFixture(): ReactNode {
  const scrollRootRef = useRef<HTMLDivElement>(null)
  const diffState: GitReviewDiffState = {
    status: 'ready',
    value: {
      fileId: 'anchor',
      patch: ANCHOR_PATCH,
      snapshotId: 'snapshot-anchor',
      status: 'ready'
    }
  }
  const fileContentState: GitReviewFileContentState = {
    status: 'ready',
    value: {
      afterText: ANCHOR_NEW_LINES.join('\n'),
      beforeText: ANCHOR_OLD_LINES.join('\n'),
      fileId: 'anchor',
      snapshotId: 'snapshot-anchor',
      status: 'ready'
    }
  }
  return (
    <div className="git-review" style={{ display: 'block', height: 220, width: 440 }}>
      <div
        className="git-review__content"
        data-testid="anchor-scrollport"
        ref={scrollRootRef}
        style={{ height: 220 }}
      >
        <GitReviewDiffCard
          diffState={diffState}
          file={createFile('anchor')}
          fileContentState={fileContentState}
          isExpanded
          isNearViewport
          isReviewActive
          isSelected
          isVisible
          loadFullFiles
          mutationLocked={false}
          mutationPending={false}
          onMutate={noop}
          onOpenFile={noop}
          onRequestDiff={noop}
          onRestore={noop}
          onToggle={noop}
          targetKind="unstaged"
          scrollRootRef={scrollRootRef}
          t={translate}
          viewMode="unified"
          wrapLines={false}
        />
        <div aria-hidden="true" style={{ height: 320 }} />
      </div>
    </div>
  )
}

async function nextPaint(): Promise<void> {
  await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
}

describe('GitReviewDiffCard browser layout', () => {
  it('keeps last-turn file cards read-only while preserving review controls', async () => {
    const screen = await render(<LastTurnCardFixture />)
    const actions = screen.container.querySelector('.git-review__file-actions')
    expect(actions).not.toBeNull()
    expect(actions?.querySelector('[aria-label="gitReview.file.restore"]')).toBeNull()
    expect(actions?.querySelector('[aria-label="gitReview.file.stage"]')).toBeNull()
    expect(actions?.querySelector('[aria-label="gitReview.file.unstage"]')).toBeNull()
    expect(actions?.querySelector('[aria-label="gitReview.file.expand"]')).not.toBeNull()
    expect(actions?.querySelector('[aria-label="gitReview.file.open"]')).not.toBeNull()
  })

  it('opens the selected review file through the tab action', async () => {
    const onOpenFile = vi.fn()
    const screen = await render(<LastTurnCardFixture onOpenFile={onOpenFile} />)

    await screen.getByRole('button', { name: 'gitReview.file.open' }).click()

    expect(onOpenFile).toHaveBeenCalledOnce()
    expect(onOpenFile).toHaveBeenCalledWith('src/last-turn.ts')
  })

  it('keeps the following hunk anchored while expanding omitted lines upward', async () => {
    const screen = await render(<ExpansionAnchorFixture />)
    const scrollport = screen.getByTestId('anchor-scrollport').element() as HTMLElement
    const anchor = screen.container.querySelector<HTMLElement>('[data-primary-anchor="true"]')
    const expandUp = screen.container.querySelector<HTMLButtonElement>(
      '[data-gap-id="gap:leading:0:0:47"] button[aria-label="gitReview.diff.expandUp"]'
    )
    if (!anchor || !expandUp) throw new Error('Expansion anchor fixture did not render')
    const beforeTop = anchor.getBoundingClientRect().top

    expandUp.click()
    await nextPaint()

    expect(scrollport.scrollTop).toBeGreaterThan(100)
    expect(Math.abs(anchor.getBoundingClientRect().top - beforeTop)).toBeLessThanOrEqual(1)
  })

  it('keeps a file header pinned until the following file pushes it away', async () => {
    const screen = await render(<StickyCardsFixture />)
    const scrollport = screen.getByTestId('review-scrollport').element() as HTMLElement
    const cards = Array.from(
      screen.container.querySelectorAll<HTMLElement>('.git-review__diff-card')
    )
    const headers = Array.from(
      screen.container.querySelectorAll<HTMLElement>('.git-review__diff-card-header')
    )

    expect(cards).toHaveLength(3)
    expect(headers).toHaveLength(3)

    scrollport.scrollTop = 100
    await nextPaint()

    const scrollportTop = scrollport.getBoundingClientRect().top
    expect(Math.abs(headers[0].getBoundingClientRect().top - scrollportTop)).toBeLessThanOrEqual(1)

    scrollport.scrollTop = cards[1].offsetTop - 12
    await nextPaint()

    const firstAtBoundary = headers[0].getBoundingClientRect()
    const secondAtBoundary = headers[1].getBoundingClientRect()
    expect(firstAtBoundary.bottom).toBeLessThanOrEqual(secondAtBoundary.top + 1)
    expect(Math.abs(secondAtBoundary.top - scrollportTop - 12)).toBeLessThanOrEqual(1)

    scrollport.scrollTop = cards[1].offsetTop + 40
    await nextPaint()

    const firstAfterBoundary = headers[0].getBoundingClientRect()
    const secondAfterBoundary = headers[1].getBoundingClientRect()
    expect(Math.abs(secondAfterBoundary.top - scrollportTop)).toBeLessThanOrEqual(1)
    expect(firstAfterBoundary.bottom).toBeLessThanOrEqual(scrollportTop + 1)
  })

  it('releases inactive diff DOM while preserving the measured scroll geometry', async () => {
    const screen = await render(<WindowedCardFixture active near />)
    await nextPaint()
    const liveBody = screen.container.querySelector<HTMLElement>('.git-review__diff-card-body')
    if (!liveBody) throw new Error('Windowed diff body did not render')
    const liveHeight = liveBody.getBoundingClientRect().height
    expect(liveHeight).toBeGreaterThan(300)
    expect(screen.container.querySelector('.git-review__unified-diff')).not.toBeNull()

    await screen.rerender(<WindowedCardFixture active={false} near={false} />)
    await nextPaint()
    const placeholder = screen.container.querySelector<HTMLElement>(
      '.git-review__diff-card-body--placeholder'
    )
    if (!placeholder) throw new Error('Windowed diff placeholder did not render')
    expect(screen.container.querySelector('.git-review__unified-diff')).toBeNull()
    expect(Math.abs(placeholder.getBoundingClientRect().height - liveHeight)).toBeLessThanOrEqual(1)

    await screen.rerender(
      <WindowedCardFixture active={false} layoutWidth={620} near={false} viewMode="split" />
    )
    await nextPaint()
    const resizedPlaceholder = screen.container.querySelector<HTMLElement>(
      '.git-review__diff-card-body--placeholder'
    )
    if (!resizedPlaceholder) throw new Error('Resized diff placeholder did not render')
    expect(
      Math.abs(resizedPlaceholder.getBoundingClientRect().height - liveHeight)
    ).toBeLessThanOrEqual(1)

    await screen.rerender(<WindowedCardFixture active near />)
    await nextPaint()
    expect(screen.container.querySelector('.git-review__unified-diff')).not.toBeNull()
  })

  it('portals and repositions a tooltip outside an overflow-clipped anchor container', async () => {
    const screen = await render(
      <div
        data-testid="clipped-container"
        style={{
          height: '30px',
          left: 0,
          overflow: 'hidden',
          position: 'fixed',
          top: 0,
          width: '180px'
        }}
      >
        <Tooltip content="Portaled tooltip" delayMs={0} describeTrigger preferredPlacement="top">
          <button style={{ height: '24px' }} type="button">
            Tooltip anchor
          </button>
        </Tooltip>
      </div>
    )

    const anchor = screen.getByRole('button', { name: 'Tooltip anchor' })
    ;(anchor.element() as HTMLButtonElement).focus()

    const tooltip = screen.getByRole('tooltip')
    await expect.element(tooltip).toHaveAttribute('data-positioned', 'true')
    await expect.element(tooltip).toBeVisible()

    const tooltipElement = tooltip.element() as HTMLElement
    const clippedContainer = screen.getByTestId('clipped-container').element() as HTMLElement
    const tooltipRect = tooltipElement.getBoundingClientRect()
    const clippedRect = clippedContainer.getBoundingClientRect()
    const backgroundColor = getComputedStyle(tooltipElement).backgroundColor

    expect(tooltipElement.parentElement).toBe(document.body)
    expect(anchor.element().getAttribute('aria-describedby')).toBe(tooltipElement.id)
    expect(tooltipElement.dataset.placement).toBe('bottom')
    expect(tooltipRect.top).toBeGreaterThanOrEqual(8)
    expect(tooltipRect.left).toBeGreaterThanOrEqual(8)
    expect(tooltipRect.right).toBeLessThanOrEqual(window.innerWidth - 8)
    expect(tooltipRect.bottom).toBeLessThanOrEqual(window.innerHeight - 8)
    expect(tooltipRect.bottom).toBeGreaterThan(clippedRect.bottom)
    expect(backgroundColor).not.toBe('rgba(0, 0, 0, 0)')

    clippedContainer.dispatchEvent(new Event('scroll'))
    await expect.poll(() => document.querySelector('.mc-tooltip')).toBeNull()
  })

  it('cancels a delayed hover tooltip when scrolling starts before it opens', async () => {
    const screen = await render(
      <div data-testid="pending-tooltip-scrollport">
        <Tooltip content="Late tooltip" delayMs={60}>
          <button type="button">Delayed anchor</button>
        </Tooltip>
      </div>
    )

    await screen.getByRole('button', { name: 'Delayed anchor' }).hover()
    screen.getByTestId('pending-tooltip-scrollport').element().dispatchEvent(new Event('scroll'))
    await new Promise((resolve) => window.setTimeout(resolve, 90))

    expect(document.querySelector('.mc-tooltip')).toBeNull()
  })
})
