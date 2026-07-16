import type { GitReviewFile, GitReviewFileStatus } from '@mycopilot/protocol'
import { useState, type CSSProperties, type ReactNode } from 'react'
import { describe, expect, it } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendTheme, type FrontendThemeId } from '../../../config/frontendTheme'
import { getGitReviewCssVariables } from '../../../config/themes/gitReviewTheme'
import type { Translate } from '../../../config/translationFormat'
import '../../../styles/global.css'
import { GitReviewDiffRenderer } from '../GitReviewDiffRenderer'
import { reduceGitDiffExpansion, type GitDiffExpansionState } from '../diff'
import '../GitReviewPanel.css'
import type { GitReviewViewMode } from '../gitReviewViewMode'
import type { GitReviewDiffState, GitReviewFileContentState } from '../useGitReview'

const LONG_OLD_LINE = `old-${'veryLongIdentifier'.repeat(30)}`
const LONG_NEW_LINE = `new-${'replacementIdentifier'.repeat(30)}`
const FIXTURE_FILE: GitReviewFile = {
  id: 'fixture.ts',
  path: 'src/fixture.ts',
  status: 'modified'
}
const PATCH_REPLACEMENT_LONG = [
  'diff --git a/src/long.ts b/src/long.ts',
  '--- a/src/long.ts',
  '+++ b/src/long.ts',
  '@@ -1,2 +1,2 @@',
  `-${LONG_OLD_LINE}`,
  `+${LONG_NEW_LINE}`,
  ' short context'
].join('\n')
const PATCH_ADDED_LONG = [
  'diff --git a/src/new.ts b/src/new.ts',
  'new file mode 100644',
  '--- /dev/null',
  '+++ b/src/new.ts',
  '@@ -0,0 +1,1 @@',
  `+${LONG_NEW_LINE}`
].join('\n')
const PATCH_DELETED_LONG = [
  'diff --git a/src/old.ts b/src/old.ts',
  'deleted file mode 100644',
  '--- a/src/old.ts',
  '+++ /dev/null',
  '@@ -1,1 +0,0 @@',
  `-${LONG_OLD_LINE}`
].join('\n')
const PATCH_MODIFIED_ADDITION_ONLY = [
  'diff --git a/src/modified.ts b/src/modified.ts',
  '--- a/src/modified.ts',
  '+++ b/src/modified.ts',
  '@@ -1,1 +1,2 @@',
  ' context',
  `+${LONG_NEW_LINE}`
].join('\n')
const PATCH_MODIFIED_DELETION_ONLY = [
  'diff --git a/src/modified.ts b/src/modified.ts',
  '--- a/src/modified.ts',
  '+++ b/src/modified.ts',
  '@@ -1,2 +1,1 @@',
  ' context',
  `-${LONG_OLD_LINE}`
].join('\n')
const PATCH_REPLACEMENT_SHORT = ['@@ -1,1 +1,1 @@', '-short old line', '+short new line'].join('\n')
const PATCH_SYNTAX_HIGHLIGHT = [
  '@@ -1,2 +1,2 @@',
  '-const answer: number = 41',
  '+const answer: number = 42',
  ' console.log("ready")'
].join('\n')
const PATCH_ASYMMETRIC_WRAP = [
  '@@ -1,2 +1,2 @@',
  '-short old line',
  `+${LONG_NEW_LINE}`,
  ' aligned context'
].join('\n')
const PATCH_ASYMMETRIC_SCROLL = [
  '@@ -1,2 +1,2 @@',
  '-short old line',
  `+${LONG_NEW_LINE}`,
  ' aligned context'
].join('\n')

const PATCH_THREE_ADDITIONS = [
  '@@ -1,1 +1,4 @@',
  ' context',
  '+first added line',
  '+second added line',
  '+third added line'
].join('\n')

const PATCH_VISUAL_GUTTERS = [
  '@@ -1,3 +1,4 @@',
  ' context before visual change',
  '-deleted visual change',
  '+added visual change',
  '+one-sided visual change',
  ' context after visual change'
].join('\n')

const LONG_ONE_SIDED_RUN_LENGTH = 128

function buildLongOneSidedPatch(kind: 'addition' | 'deletion'): string {
  const changedLines = Array.from(
    { length: LONG_ONE_SIDED_RUN_LENGTH },
    (_, index) => `${kind === 'addition' ? '+' : '-'}long ${kind} ${index + 1}`
  )
  const oldCount = kind === 'addition' ? 3 : LONG_ONE_SIDED_RUN_LENGTH + 3
  const newCount = kind === 'addition' ? LONG_ONE_SIDED_RUN_LENGTH + 3 : 3
  const nextOldStart = oldCount + 100
  const nextNewStart = newCount + 100

  return [
    `@@ -1,${oldCount} +1,${newCount} @@`,
    ' context before long run',
    ...changedLines,
    ' context immediately after long run',
    ' context at end of first hunk',
    `@@ -${nextOldStart},2 +${nextNewStart},2 @@`,
    ' next hunk anchor context',
    '-old next hunk value',
    '+new next hunk value'
  ].join('\n')
}

const PATCH_SPLIT_VERTICAL_ALIGNMENT = [
  '@@ -1,3 +1,4 @@',
  ' line 1',
  '+first inserted line',
  ' line 2',
  ' line 3',
  '@@ -10,3 +11,10 @@',
  ' line 10',
  '+inserted line 11',
  '+inserted line 12',
  '+inserted line 13',
  '+inserted line 14',
  '+inserted line 15',
  '+inserted line 16',
  '+inserted line 17',
  ' line 11',
  ' line 12',
  '@@ -20,2 +28,2 @@',
  ' line 20',
  '-old line 21',
  '+new line 29'
].join('\n')

const FULL_OLD_LINES = Array.from({ length: 80 }, (_, index) => `line ${index + 1}`)
const FULL_NEW_LINES = FULL_OLD_LINES.map((line, index) =>
  index === 4 || index === 55 ? `${line} changed` : line
)
const PATCH_WITH_OMITTED_RANGES = [
  '@@ -3,5 +3,5 @@',
  ' line 3',
  ' line 4',
  '-line 5',
  '+line 5 changed',
  ' line 6',
  ' line 7',
  '@@ -54,5 +54,5 @@',
  ' line 54',
  ' line 55',
  '-line 56',
  '+line 56 changed',
  ' line 57',
  ' line 58'
].join('\n')

const translate: Translate = (key) => {
  if (key === 'gitReview.diff.unmodifiedLines') return '{count} unmodified lines'
  if (key === 'gitReview.diff.expandDown') return 'Expand {count} lines below'
  if (key === 'gitReview.diff.expandUp') return 'Expand {count} lines above'
  if (key === 'gitReview.diff.expandBoth') return 'Expand from both ends'
  return key
}

const fullContentState: GitReviewFileContentState = {
  status: 'ready',
  value: {
    afterText: FULL_NEW_LINES.join('\n'),
    beforeText: FULL_OLD_LINES.join('\n'),
    fileId: 'fixture.ts',
    snapshotId: 'snapshot-1',
    status: 'ready'
  }
}

function createReadyDiffState(patch: string): GitReviewDiffState {
  return {
    status: 'ready',
    value: {
      fileId: 'fixture.ts',
      patch,
      snapshotId: 'snapshot-1',
      status: 'ready'
    }
  }
}

function InteractiveDiffFixture({
  viewMode = 'split',
  wrapLines = false
}: {
  viewMode?: GitReviewViewMode
  wrapLines?: boolean
}): ReactNode {
  const [expansionState, setExpansionState] = useState<GitDiffExpansionState>(() => new Map())
  return (
    <div className="git-review" style={{ display: 'block', width: 520 }}>
      <div className="git-review__diff-card-body" data-testid="interactive-diff">
        <GitReviewDiffRenderer
          diffState={createReadyDiffState(PATCH_WITH_OMITTED_RANGES)}
          expansionState={expansionState}
          file={FIXTURE_FILE}
          fileContentState={fullContentState}
          onExpand={(action) =>
            setExpansionState((current) => reduceGitDiffExpansion(current, action))
          }
          onRequestDiff={() => undefined}
          syntaxHighlightingEnabled={false}
          t={translate}
          viewMode={viewMode}
          wrapLines={wrapLines}
        />
      </div>
    </div>
  )
}

interface DiffFixtureProps {
  fileContentState?: GitReviewFileContentState
  fileStatus?: GitReviewFileStatus
  patch?: string
  syntaxHighlightingEnabled?: boolean
  syntaxThemeId?: FrontendThemeId
  viewMode?: GitReviewViewMode
  width?: number
  wrapLines?: boolean
  zoom?: number
}

function DiffFixture({
  fileContentState,
  fileStatus = 'modified',
  patch = PATCH_REPLACEMENT_LONG,
  syntaxHighlightingEnabled = false,
  syntaxThemeId = 'classic-dark',
  viewMode = 'split',
  width = 420,
  wrapLines = false,
  zoom = 1
}: DiffFixtureProps): ReactNode {
  const style = {
    ...getGitReviewCssVariables(getFrontendTheme(syntaxThemeId).tokens.colors.gitReview),
    '--mc-font-size-sm': '12px',
    '--mc-color-border-hairline': 'rgb(72 78 84)',
    '--mc-color-diff-addition-text': 'rgb(156 204 106)',
    '--mc-color-diff-deletion-text': 'rgb(244 117 121)',
    '--mc-color-surface-muted': 'rgb(78 85 91)',
    '--mc-color-surface-right-panel': 'rgb(38 43 48)',
    '--mc-color-text-muted': 'rgb(155 159 163)',
    display: 'block',
    height: 'auto',
    width: `${width}px`,
    zoom
  } as CSSProperties

  return (
    <div className="git-review" style={style}>
      <div className="git-review__diff-card-body" data-testid="diff-viewport">
        <GitReviewDiffRenderer
          diffState={createReadyDiffState(patch)}
          file={{ ...FIXTURE_FILE, status: fileStatus }}
          fileContentState={fileContentState}
          onRequestDiff={() => undefined}
          syntaxHighlightingEnabled={syntaxHighlightingEnabled}
          t={translate}
          viewMode={viewMode}
          wrapLines={wrapLines}
        />
      </div>
    </div>
  )
}

function expectWidthCloseTo(actual: number, expected: number, tolerance = 1): void {
  expect(Math.abs(actual - expected)).toBeLessThanOrEqual(tolerance)
}

function findSplitLineByContent(pane: HTMLElement, content: string): HTMLElement {
  const line = Array.from(pane.querySelectorAll<HTMLElement>('.git-review__split-pane-line')).find(
    (candidate) =>
      candidate.querySelector<HTMLElement>('.git-review__diff-content')?.textContent === content
  )
  if (!line) throw new Error(`Missing split line with content: ${content}`)
  return line
}

function expectRectsVerticallyAligned(
  left: HTMLElement,
  right: HTMLElement,
  tolerance = 0.35
): void {
  const leftRect = left.getBoundingClientRect()
  const rightRect = right.getBoundingClientRect()
  expectWidthCloseTo(leftRect.top, rightRect.top, tolerance)
  expectWidthCloseTo(leftRect.bottom, rightRect.bottom, tolerance)
}

function colorChannels(color: string): readonly [number, number, number, number] {
  const canvas = document.createElement('canvas')
  canvas.width = 1
  canvas.height = 1
  const context = canvas.getContext('2d')
  if (!context) throw new Error('A 2D canvas context is required to inspect rendered colors')
  context.clearRect(0, 0, 1, 1)
  context.fillStyle = color
  context.fillRect(0, 0, 1, 1)
  const [red = 0, green = 0, blue = 0, alpha = 0] = context.getImageData(0, 0, 1, 1).data
  return [red, green, blue, alpha]
}

function colorDistance(left: string, right: string): number {
  const [leftRed, leftGreen, leftBlue] = colorChannels(left)
  const [rightRed, rightGreen, rightBlue] = colorChannels(right)
  return Math.hypot(leftRed - rightRed, leftGreen - rightGreen, leftBlue - rightBlue)
}

function hasVisibleLeadingRail(gutter: HTMLElement): boolean {
  const style = getComputedStyle(gutter)
  const before = getComputedStyle(gutter, '::before')
  const borderWidth = Number.parseFloat(style.borderLeftWidth)
  const beforeWidth = Number.parseFloat(before.width)
  const [, , , borderAlpha] = colorChannels(style.borderLeftColor)
  const [, , , beforeAlpha] = colorChannels(before.backgroundColor)
  const hasBorder = borderWidth >= 3 && style.borderLeftStyle !== 'none' && borderAlpha > 0
  const hasPseudoElement =
    before.content !== 'none' &&
    before.display !== 'none' &&
    beforeWidth >= 3 &&
    (beforeAlpha > 0 || before.backgroundImage !== 'none')
  const shadowLengths = Array.from(style.boxShadow.matchAll(/-?\d+(?:\.\d+)?px/g), (match) =>
    Math.abs(Number.parseFloat(match[0]))
  )
  const hasInsetShadow =
    style.boxShadow !== 'none' &&
    style.boxShadow.includes('inset') &&
    shadowLengths.some((length) => length >= 3)
  return hasBorder || hasPseudoElement || hasInsetShadow
}

function clickMiddleGapControl(container: HTMLElement, accessibleName: string): void {
  const selector = `[data-diff-pane="old"] [data-gap-id="gap:between:7:7:46"] button[aria-label="${accessibleName}"]`
  const button = container.querySelector<HTMLButtonElement>(selector)
  if (!button) throw new Error(`Missing middle-gap control: ${accessibleName}`)
  button.click()
}

describe('GitReviewDiffRenderer split layout', () => {
  it.each([420, 641])('keeps two fixed half-width scrollports at %ipx', async (width) => {
    const screen = await render(<DiffFixture width={width} />)
    const viewport = screen.getByTestId('diff-viewport').element() as HTMLElement
    const root = screen.container.querySelector<HTMLElement>('.git-review__split-diff--scrollable')
    const panes = Array.from(
      screen.container.querySelectorAll<HTMLElement>('.git-review__split-pane')
    )
    if (!root) throw new Error('Scrollable split root did not render')

    expect(panes).toHaveLength(2)
    const rootRect = root.getBoundingClientRect()
    const leftRect = panes[0].getBoundingClientRect()
    const rightRect = panes[1].getBoundingClientRect()
    expectWidthCloseTo(leftRect.width, rightRect.width)
    expectWidthCloseTo(leftRect.width + rightRect.width, rootRect.width, 2)
    expectWidthCloseTo(leftRect.width, rootRect.width / 2, 1)
    expectWidthCloseTo(rootRect.width, viewport.getBoundingClientRect().width)
    expect(viewport.scrollWidth).toBeLessThanOrEqual(viewport.clientWidth + 1)
    for (const pane of panes) {
      expect(pane.scrollWidth).toBeGreaterThan(pane.clientWidth + 20)
    }
  })

  it('synchronizes native pane scrolling in both directions within the same frame', async () => {
    const screen = await render(<DiffFixture />)
    const oldPane = screen.container.querySelector<HTMLElement>('[data-diff-pane="old"]')
    const newPane = screen.container.querySelector<HTMLElement>('[data-diff-pane="new"]')
    if (!oldPane || !newPane) throw new Error('Both split panes are required')

    oldPane.scrollLeft = 80
    oldPane.dispatchEvent(new Event('scroll', { bubbles: true }))
    expect(newPane.scrollLeft).toBe(80)

    newPane.scrollLeft = 34
    newPane.dispatchEvent(new Event('scroll', { bubbles: true }))
    expect(oldPane.scrollLeft).toBe(34)
  })

  it('gives asymmetric panes the same maximum scroll range', async () => {
    const screen = await render(<DiffFixture patch={PATCH_ASYMMETRIC_SCROLL} />)
    const oldPane = screen.container.querySelector<HTMLElement>('[data-diff-pane="old"]')
    const newPane = screen.container.querySelector<HTMLElement>('[data-diff-pane="new"]')
    if (!oldPane || !newPane) throw new Error('Both split panes are required')

    const oldMaximum = oldPane.scrollWidth - oldPane.clientWidth
    const newMaximum = newPane.scrollWidth - newPane.clientWidth
    expect(oldMaximum).toBeGreaterThan(20)
    expectWidthCloseTo(oldMaximum, newMaximum)

    newPane.scrollLeft = newMaximum
    newPane.dispatchEvent(new Event('scroll', { bubbles: true }))
    await expect.poll(() => oldPane.scrollLeft).toBe(newMaximum)
  })

  it('remeasures the shared scroll range when the patch becomes shorter', async () => {
    const screen = await render(<DiffFixture />)
    const oldPane = screen.container.querySelector<HTMLElement>('[data-diff-pane="old"]')
    const newPane = screen.container.querySelector<HTMLElement>('[data-diff-pane="new"]')
    if (!oldPane || !newPane) throw new Error('Both split panes are required')
    expect(oldPane.scrollWidth).toBeGreaterThan(oldPane.clientWidth + 20)

    await screen.rerender(<DiffFixture patch={PATCH_REPLACEMENT_SHORT} />)
    const updatedOldPane = screen.container.querySelector<HTMLElement>('[data-diff-pane="old"]')
    const updatedNewPane = screen.container.querySelector<HTMLElement>('[data-diff-pane="new"]')
    if (!updatedOldPane || !updatedNewPane) throw new Error('Updated split panes are required')
    await expect
      .poll(() => updatedOldPane.scrollWidth)
      .toBeLessThanOrEqual(updatedOldPane.clientWidth + 1)
    expect(updatedNewPane.scrollWidth).toBeLessThanOrEqual(updatedNewPane.clientWidth + 1)
  })

  it.each([
    ['added', PATCH_ADDED_LONG, 'new'],
    ['untracked', PATCH_ADDED_LONG, 'new'],
    ['deleted', PATCH_DELETED_LONG, 'old']
  ] as const)('renders %s files as one full-width %s-side pane', async (status, patch, side) => {
    const screen = await render(<DiffFixture fileStatus={status} patch={patch} />)
    const viewport = screen.getByTestId('diff-viewport').element() as HTMLElement
    const root = screen.container.querySelector<HTMLElement>('.git-review__single-diff')
    if (!root) throw new Error('Single-sided diff did not render')

    expect(root.dataset.side).toBe(side)
    expect(screen.container.querySelectorAll('.git-review__split-pane')).toHaveLength(0)
    expectWidthCloseTo(root.getBoundingClientRect().width, viewport.getBoundingClientRect().width)
    expect(root.scrollWidth).toBeGreaterThan(root.clientWidth + 20)
    expect(
      root.querySelectorAll(
        side === 'old' ? '.git-review__diff-line--addition' : '.git-review__diff-line--deletion'
      )
    ).toHaveLength(0)
  })

  it.each([PATCH_MODIFIED_ADDITION_ONLY, PATCH_MODIFIED_DELETION_ONLY])(
    'keeps a modified one-way hunk two-sided',
    async (patch) => {
      const screen = await render(<DiffFixture fileStatus="modified" patch={patch} />)
      expect(screen.container.querySelectorAll('.git-review__split-pane')).toHaveLength(2)
      expect(screen.container.querySelector('.git-review__single-diff')).toBeNull()
    }
  )

  it('renders one continuous striped buffer for a consecutive missing-side run', async () => {
    const screen = await render(<DiffFixture patch={PATCH_THREE_ADDITIONS} />)
    const buffers = screen.container.querySelectorAll<HTMLElement>('[data-content-buffer="true"]')
    const additions = screen.container.querySelectorAll<HTMLElement>(
      '[data-diff-pane="new"] .git-review__diff-line--addition'
    )

    expect(buffers).toHaveLength(1)
    expect(additions).toHaveLength(3)
    const additionHeight = Array.from(additions).reduce(
      (height, line) => height + line.getBoundingClientRect().height,
      0
    )
    expectWidthCloseTo(buffers[0].getBoundingClientRect().height, additionHeight, 1)
    expect(
      getComputedStyle(buffers[0].querySelector('.git-review__split-buffer-content')!)
        .backgroundImage
    ).toContain('repeating-linear-gradient')
  })

  it.each(
    ([0.8, 1, 1.25] as const).flatMap((zoom) => [
      { kind: 'addition' as const, missingSide: 'old', presentSide: 'new', zoom },
      { kind: 'deletion' as const, missingSide: 'new', presentSide: 'old', zoom }
    ])
  )(
    'keeps a 128-line $kind buffer and every following anchor aligned at $zoom zoom',
    async ({ kind, missingSide, presentSide, zoom }) => {
      const screen = await render(
        <DiffFixture patch={buildLongOneSidedPatch(kind)} width={641} zoom={zoom} />
      )
      const missingPane = screen.container.querySelector<HTMLElement>(
        `[data-diff-pane="${missingSide}"]`
      )
      const presentPane = screen.container.querySelector<HTMLElement>(
        `[data-diff-pane="${presentSide}"]`
      )
      if (!missingPane || !presentPane) throw new Error('Both split panes are required')

      const buffer = missingPane.querySelector<HTMLElement>('[data-content-buffer="true"]')
      const changedLines = Array.from(
        presentPane.querySelectorAll<HTMLElement>(`.git-review__diff-line--${kind}`)
      ).filter((line) =>
        line.querySelector('.git-review__diff-content')?.textContent?.startsWith(`long ${kind}`)
      )
      if (!buffer || !changedLines[0] || !changedLines.at(-1)) {
        throw new Error('The long one-sided run did not render')
      }

      expect(changedLines).toHaveLength(LONG_ONE_SIDED_RUN_LENGTH)
      const bufferRect = buffer.getBoundingClientRect()
      const firstChangedRect = changedLines[0].getBoundingClientRect()
      const lastChangedRect = changedLines.at(-1)!.getBoundingClientRect()
      expectWidthCloseTo(bufferRect.top, firstChangedRect.top, 0.35)
      expectWidthCloseTo(bufferRect.bottom, lastChangedRect.bottom, 0.35)

      const missingContext = findSplitLineByContent(
        missingPane,
        'context immediately after long run'
      )
      const presentContext = findSplitLineByContent(
        presentPane,
        'context immediately after long run'
      )
      expectRectsVerticallyAligned(missingContext, presentContext)

      const missingAnchors = missingPane.querySelectorAll<HTMLElement>('[data-diff-anchor-id]')
      const presentAnchors = presentPane.querySelectorAll<HTMLElement>('[data-diff-anchor-id]')
      expect(missingAnchors).toHaveLength(2)
      expect(presentAnchors).toHaveLength(2)
      expectRectsVerticallyAligned(missingAnchors[1]!, presentAnchors[1]!)
      expectWidthCloseTo(missingPane.scrollHeight, presentPane.scrollHeight, 0.35)
    }
  )

  it('colors changed gutters and renders a leading rail plus a darker buffer gutter', async () => {
    const screen = await render(<DiffFixture patch={PATCH_VISUAL_GUTTERS} />)
    const oldPane = screen.container.querySelector<HTMLElement>('[data-diff-pane="old"]')
    const newPane = screen.container.querySelector<HTMLElement>('[data-diff-pane="new"]')
    if (!oldPane || !newPane) throw new Error('Both split panes are required')

    const oldContextGutter = findSplitLineByContent(
      oldPane,
      'context before visual change'
    ).querySelector<HTMLElement>('.git-review__line-number')
    const newContextGutter = findSplitLineByContent(
      newPane,
      'context before visual change'
    ).querySelector<HTMLElement>('.git-review__line-number')
    const deletionGutter = findSplitLineByContent(
      oldPane,
      'deleted visual change'
    ).querySelector<HTMLElement>('.git-review__line-number')
    const additionGutter = findSplitLineByContent(
      newPane,
      'added visual change'
    ).querySelector<HTMLElement>('.git-review__line-number')
    const bufferGutter = oldPane.querySelector<HTMLElement>('.git-review__split-buffer-gutter')
    const bufferContent = oldPane.querySelector<HTMLElement>('.git-review__split-buffer-content')
    if (
      !oldContextGutter ||
      !newContextGutter ||
      !deletionGutter ||
      !additionGutter ||
      !bufferGutter ||
      !bufferContent
    ) {
      throw new Error('Expected changed-line and buffer gutters did not render')
    }

    const oldContextStyle = getComputedStyle(oldContextGutter)
    const newContextStyle = getComputedStyle(newContextGutter)
    const deletionStyle = getComputedStyle(deletionGutter)
    const additionStyle = getComputedStyle(additionGutter)
    expect(deletionStyle.color).not.toBe(oldContextStyle.color)
    expect(deletionStyle.backgroundColor).not.toBe(oldContextStyle.backgroundColor)
    expect(additionStyle.color).not.toBe(newContextStyle.color)
    expect(additionStyle.backgroundColor).not.toBe(newContextStyle.backgroundColor)
    expect(hasVisibleLeadingRail(deletionGutter)).toBe(true)
    expect(hasVisibleLeadingRail(additionGutter)).toBe(true)
    expect(hasVisibleLeadingRail(oldContextGutter)).toBe(false)
    expect(hasVisibleLeadingRail(newContextGutter)).toBe(false)

    const bufferGutterColor = getComputedStyle(bufferGutter).backgroundColor
    const bufferContentColor = getComputedStyle(bufferContent).backgroundColor
    expect(bufferGutterColor).not.toBe(bufferContentColor)
    expect(colorDistance(bufferGutterColor, bufferContentColor)).toBeGreaterThan(3)
  })

  it('keeps both independent panes vertically aligned after one-sided blocks', async () => {
    const screen = await render(<DiffFixture patch={PATCH_SPLIT_VERTICAL_ALIGNMENT} />)
    const oldPane = screen.container.querySelector<HTMLElement>('[data-diff-pane="old"]')
    const newPane = screen.container.querySelector<HTMLElement>('[data-diff-pane="new"]')
    const oldThirdHunk = oldPane?.querySelectorAll<HTMLElement>('[data-diff-anchor-id]')[2]
    const newThirdHunk = newPane?.querySelectorAll<HTMLElement>('[data-diff-anchor-id]')[2]
    if (!oldPane || !newPane || !oldThirdHunk || !newThirdHunk) {
      throw new Error('Aligned split fixture did not render')
    }

    expectWidthCloseTo(
      oldThirdHunk.getBoundingClientRect().top,
      newThirdHunk.getBoundingClientRect().top,
      0.5
    )
    expectWidthCloseTo(oldPane.scrollHeight, newPane.scrollHeight, 0.5)

    const oldGaps = Array.from(oldPane.querySelectorAll<HTMLElement>('.git-review__gap-separator'))
    const newGaps = Array.from(newPane.querySelectorAll<HTMLElement>('.git-review__gap-separator'))
    expect(oldGaps).toHaveLength(newGaps.length)
    oldGaps.forEach((oldGap, index) => {
      const newGap = newGaps[index]
      if (!newGap) throw new Error('The matching new-side separator is missing')
      expectWidthCloseTo(
        oldGap.getBoundingClientRect().top,
        newGap.getBoundingClientRect().top,
        0.5
      )
      expectWidthCloseTo(
        oldGap.getBoundingClientRect().height,
        newGap.getBoundingClientRect().height,
        0.5
      )
    })
  })
})

describe('GitReviewDiffRenderer omitted context', () => {
  it('replaces raw hunk headers with one shared semantic separator in split view', async () => {
    const screen = await render(<DiffFixture patch={PATCH_WITH_OMITTED_RANGES} />)

    expect(screen.container.textContent).not.toContain('@@')
    expect(screen.container.querySelectorAll('.git-review__gap-label')).toHaveLength(2)
    expect(screen.container.querySelectorAll('[data-gap-id="gap:between:7:7:46"]')).toHaveLength(2)
    expect(screen.getByText('46 unmodified lines')).toBeTruthy()
    expect(screen.container.querySelectorAll('.git-review__gap-gutter button')).toHaveLength(0)
  })

  it('keeps the shared omitted-range separator at the compact Codex row height', async () => {
    const screen = await render(<InteractiveDiffFixture />)
    const separator = screen.container.querySelector<HTMLElement>(
      '[data-diff-pane="old"] [data-gap-id="gap:between:7:7:46"] .git-review__gap-separator'
    )
    if (!separator) throw new Error('Middle gap separator did not render')

    expectWidthCloseTo(separator.getBoundingClientRect().height, 32, 0.5)
  })

  it('expands a hydrated middle gap by 20 lines in either direction', async () => {
    const screen = await render(<InteractiveDiffFixture />)

    clickMiddleGapControl(screen.container, 'Expand 20 lines below')
    await expect.element(screen.getByText('26 unmodified lines')).toBeVisible()
    expect(screen.container.textContent).toContain('line 8')
    expect(screen.container.textContent).toContain('line 27')

    clickMiddleGapControl(screen.container, 'Expand 20 lines above')
    await expect.element(screen.getByText('6 unmodified lines')).toBeVisible()
    expect(screen.container.textContent).toContain('line 34')
    expect(screen.container.textContent).toContain('line 53')
  })

  it('fails closed to the compact diff when full content does not match', async () => {
    const staleContent: GitReviewFileContentState = {
      status: 'ready',
      value: {
        ...fullContentState.value,
        afterText: fullContentState.value.afterText?.replace('line 3', 'stale line 3') ?? null
      }
    }
    const screen = await render(
      <DiffFixture fileContentState={staleContent} patch={PATCH_WITH_OMITTED_RANGES} />
    )

    expect(screen.getByText('46 unmodified lines')).toBeTruthy()
    expect(screen.container.querySelectorAll('.git-review__gap-gutter button')).toHaveLength(0)
    expect(screen.container.textContent).toContain('line 3')
    expect(screen.container.textContent).not.toContain('stale line 3')
  })

  it('preserves expansion while switching layout and line wrapping', async () => {
    const screen = await render(<InteractiveDiffFixture />)
    clickMiddleGapControl(screen.container, 'Expand 20 lines below')
    await expect.element(screen.getByText('26 unmodified lines')).toBeVisible()

    await screen.rerender(<InteractiveDiffFixture viewMode="unified" wrapLines />)
    await expect.element(screen.getByText('26 unmodified lines')).toBeVisible()
    expect(screen.container.querySelector('.git-review__unified-diff')).not.toBeNull()
    expect(screen.container.textContent).toContain('line 8')
  })
})

describe('GitReviewDiffRenderer line wrapping', () => {
  it('wraps unified long lines without retaining horizontal overflow', async () => {
    const screen = await render(<DiffFixture viewMode="unified" />)
    const viewport = screen.getByTestId('diff-viewport').element() as HTMLElement
    const line = screen.container.querySelector<HTMLElement>('.git-review__unified-line')
    if (!line) throw new Error('Unified line did not render')
    const unwrappedHeight = line.getBoundingClientRect().height
    expect(viewport.scrollWidth).toBeGreaterThan(viewport.clientWidth + 20)

    await screen.rerender(<DiffFixture viewMode="unified" wrapLines />)
    const wrappedLine = screen.container.querySelector<HTMLElement>('.git-review__unified-line')
    if (!wrappedLine) throw new Error('Wrapped unified line did not render')
    expect(viewport.scrollWidth).toBeLessThanOrEqual(viewport.clientWidth + 1)
    expect(wrappedLine.getBoundingClientRect().height).toBeGreaterThan(unwrappedHeight)
  })

  it('keeps wrapped split rows half-width and aligned to the taller side', async () => {
    const screen = await render(<DiffFixture patch={PATCH_ASYMMETRIC_WRAP} wrapLines />)
    const viewport = screen.getByTestId('diff-viewport').element() as HTMLElement
    const row = screen.container.querySelector<HTMLElement>('.git-review__split-row')
    const sides = row
      ? Array.from(row.querySelectorAll<HTMLElement>('.git-review__split-side'))
      : []
    if (!row || sides.length !== 2) throw new Error('Wrapped split row did not render')

    expect(viewport.scrollWidth).toBeLessThanOrEqual(viewport.clientWidth + 1)
    expectWidthCloseTo(
      sides[0].getBoundingClientRect().width,
      sides[1].getBoundingClientRect().width
    )
    expectWidthCloseTo(
      sides[0].getBoundingClientRect().width + sides[1].getBoundingClientRect().width,
      row.getBoundingClientRect().width,
      2
    )
    expectWidthCloseTo(sides[0].getBoundingClientRect().top, sides[1].getBoundingClientRect().top)
    expectWidthCloseTo(
      sides[0].getBoundingClientRect().height,
      sides[1].getBoundingClientRect().height
    )
    expect(row.getBoundingClientRect().height).toBeGreaterThan(18)
  })

  it.each([
    ['added', PATCH_ADDED_LONG],
    ['deleted', PATCH_DELETED_LONG]
  ] as const)('wraps a full-width %s file without horizontal overflow', async (status, patch) => {
    const screen = await render(<DiffFixture fileStatus={status} patch={patch} />)
    const unwrappedLine = screen.container.querySelector<HTMLElement>('.git-review__single-line')
    if (!unwrappedLine) throw new Error('Unwrapped single-sided line did not render')
    const unwrappedHeight = unwrappedLine.getBoundingClientRect().height

    await screen.rerender(<DiffFixture fileStatus={status} patch={patch} wrapLines />)
    const viewport = screen.getByTestId('diff-viewport').element() as HTMLElement
    const root = screen.container.querySelector<HTMLElement>('.git-review__single-diff')
    const wrappedLine = screen.container.querySelector<HTMLElement>('.git-review__single-line')
    if (!root || !wrappedLine) throw new Error('Wrapped single-sided diff did not render')

    expect(viewport.scrollWidth).toBeLessThanOrEqual(viewport.clientWidth + 1)
    expectWidthCloseTo(root.getBoundingClientRect().width, viewport.getBoundingClientRect().width)
    expect(wrappedLine.getBoundingClientRect().height).toBeGreaterThan(unwrappedHeight)
  })
})

describe('GitReviewDiffRenderer syntax highlighting', () => {
  it('adds foreground-only tokens without changing the source text', async () => {
    const screen = await render(<DiffFixture patch={PATCH_SYNTAX_HIGHLIGHT} viewMode="unified" />)
    const plainAddedLine = Array.from(
      screen.container.querySelectorAll<HTMLElement>('.git-review__diff-line--addition')
    ).find(
      (line) =>
        line.querySelector('.git-review__diff-content')?.textContent === 'const answer: number = 42'
    )
    if (!plainAddedLine) throw new Error('Expected the plain addition line')
    const plainRect = plainAddedLine.getBoundingClientRect()

    await screen.rerender(
      <DiffFixture patch={PATCH_SYNTAX_HIGHLIGHT} syntaxHighlightingEnabled viewMode="unified" />
    )

    await expect
      .poll(() => screen.container.querySelectorAll('[data-syntax-highlighted="true"]').length, {
        timeout: 5_000
      })
      .toBeGreaterThan(0)

    const addedLine = Array.from(
      screen.container.querySelectorAll<HTMLElement>('.git-review__diff-line--addition')
    ).find(
      (line) =>
        line.querySelector('.git-review__diff-content')?.textContent === 'const answer: number = 42'
    )
    if (!addedLine) throw new Error('Expected the highlighted addition line')
    const content = addedLine.querySelector<HTMLElement>('.git-review__diff-content')
    const tokens = Array.from(addedLine.querySelectorAll<HTMLElement>('[data-syntax-token="true"]'))
    if (!content) throw new Error('Expected highlighted content')

    expect(content.textContent).toBe('const answer: number = 42')
    expect(tokens.length).toBeGreaterThan(1)
    expectWidthCloseTo(addedLine.getBoundingClientRect().width, plainRect.width, 0.1)
    expectWidthCloseTo(addedLine.getBoundingClientRect().height, plainRect.height, 0.1)
    expect(new Set(tokens.map((token) => getComputedStyle(token).color)).size).toBeGreaterThan(1)
    for (const token of tokens) {
      const style = getComputedStyle(token)
      expect(style.backgroundColor).toBe('rgba(0, 0, 0, 0)')
      expect(style.fontStyle).toBe('normal')
      expect(style.textDecorationLine).toBe('none')
    }

    const keywordBefore = tokens.find((token) => token.textContent === 'const')
    if (!keywordBefore) throw new Error('Expected a highlighted keyword token')
    const colorBefore = getComputedStyle(keywordBefore).color

    await screen.rerender(
      <DiffFixture
        patch={PATCH_SYNTAX_HIGHLIGHT}
        syntaxHighlightingEnabled
        syntaxThemeId="classic-light"
        viewMode="unified"
      />
    )
    const keywordAfter = Array.from(
      screen.container.querySelectorAll<HTMLElement>('[data-syntax-token="true"]')
    ).find((token) => token.textContent === 'const')

    expect(keywordAfter?.textContent).toBe(keywordBefore.textContent)
    expect(keywordAfter && getComputedStyle(keywordAfter).color).not.toBe(colorBefore)
  })
})
