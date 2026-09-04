import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { GitReviewTarget } from '@mycopilot/protocol'
import '../GitReviewPanel.css'

const { setTargetSpy } = vi.hoisted(() => ({
  setTargetSpy: vi.fn()
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    resolvedThemeId: 'classic-light',
    t: (key: string) => key
  })
}))

vi.mock('../useGitReview', async () => {
  const { useCallback, useMemo, useState } = await import('react')
  const files = [
    {
      id: 'file-1',
      path: 'src/file.ts',
      stats: { additions: 1, deletions: 1 },
      status: 'modified' as const
    },
    {
      id: 'file-2',
      path: 'src/target.ts',
      stats: { additions: 2, deletions: 0 },
      status: 'added' as const
    }
  ]
  const noop = (): void => undefined
  const noopAsync = async (): Promise<void> => undefined
  const diffStates = {
    'file-1': {
      status: 'ready' as const,
      value: {
        fileId: 'file-1',
        patch: '@@ -1 +1 @@\n-old\n+new',
        snapshotId: 'snapshot-1',
        status: 'ready' as const
      }
    }
  }

  return {
    gitReviewTargetKey: (target: GitReviewTarget) =>
      target.kind === 'lastTurn' ? `${target.kind}:${target.conversationId}` : target.kind,
    useGitReview: (
      _projectId: string,
      _isActive: boolean,
      initialTarget: GitReviewTarget = { kind: 'unstaged' }
    ) => {
      const [target, setTargetState] = useState<GitReviewTarget>(initialTarget)
      const setTarget = useCallback((nextTarget: GitReviewTarget) => {
        setTargetSpy(nextTarget)
        setTargetState(nextTarget)
      }, [])
      const summaryState = useMemo(
        () => ({
          status: 'ready' as const,
          value: {
            files,
            context: {},
            repositoryId: 'repository-1',
            target,
            snapshotId: 'snapshot-1',
            stats: {
              additions: 1,
              deletions: 1,
              fileCount: 1,
              lineCountsComplete: true
            },
            truncated: false
          }
        }),
        [target]
      )

      return {
        cancelQueuedFileContentsExcept: noop,
        cancelQueuedFileDiffsExcept: noop,
        diffStates,
        dismissMutationError: noop,
        fileContentStates: {},
        loadFileContent: noop,
        loadFileDiff: noop,
        mutateFile: noopAsync,
        mutationError: null,
        pendingFileId: null,
        refresh: noopAsync,
        retryFileDiff: noop,
        setHotDiffFileIds: noop,
        setHotFullContentFileIds: noop,
        setTarget,
        summaryState,
        target
      }
    }
  }
})

const { GitReviewPanel } = await import('../GitReviewPanel')
const NOOP = (): void => undefined

describe('Git Review scope navigation', () => {
  it('switches an existing review page to last turn once', async () => {
    setTargetSpy.mockClear()
    const screen = await render(<GitReviewPanel isActive onOpenFile={NOOP} projectId="project-1" />)

    await expect
      .element(screen.getByRole('button', { name: 'gitReview.scope.unstaged' }))
      .toBeVisible()
    await screen.rerender(
      <GitReviewPanel
        isActive
        onOpenFile={NOOP}
        projectId="project-1"
        targetNavigation={{
          requestId: 1,
          target: { kind: 'lastTurn', conversationId: 'conversation-1' }
        }}
      />
    )

    await expect
      .element(screen.getByRole('button', { name: 'gitReview.scope.lastTurn' }))
      .toBeVisible()
    expect(setTargetSpy.mock.calls).toEqual([
      [{ kind: 'lastTurn', conversationId: 'conversation-1' }]
    ])
  })

  it('keeps expanded state when an already-open last-turn page is activated again', async () => {
    setTargetSpy.mockClear()
    const renderPanel = (requestId: number) => (
      <GitReviewPanel
        isActive
        onOpenFile={NOOP}
        projectId="project-1"
        targetNavigation={{
          requestId,
          target: { kind: 'lastTurn', conversationId: 'conversation-1' }
        }}
      />
    )
    const screen = await render(renderPanel(1))
    const card = screen.container.querySelector<HTMLElement>(
      '.git-review__diff-card[data-file-id="file-1"]'
    )
    const toggle = card?.querySelector<HTMLButtonElement>('.git-review__diff-card-main')
    if (!card || !toggle) throw new Error('Review navigation fixture did not render')

    expect(card.dataset.expanded).toBeUndefined()
    toggle.click()
    await expect.poll(() => card.dataset.expanded).toBe('true')
    await screen.rerender(renderPanel(2))

    await expect.poll(() => card.dataset.expanded).toBe('true')
    expect(setTargetSpy).not.toHaveBeenCalled()
  })

  it('expands a requested file only when it exists in the last-turn summary', async () => {
    setTargetSpy.mockClear()
    const renderPanel = (requestId: number, filePath?: string) => (
      <GitReviewPanel
        isActive
        onOpenFile={NOOP}
        projectId="project-1"
        targetNavigation={{
          requestId,
          target: { kind: 'lastTurn', conversationId: 'conversation-1' },
          ...(filePath ? { filePath } : {})
        }}
      />
    )
    const screen = await render(renderPanel(1))

    const firstCard = screen.container.querySelector<HTMLElement>(
      '.git-review__diff-card[data-file-id="file-1"]'
    )
    const targetCard = screen.container.querySelector<HTMLElement>(
      '.git-review__diff-card[data-file-id="file-2"]'
    )
    if (!firstCard || !targetCard) throw new Error('Review navigation fixture did not render')
    const firstToggle = firstCard.querySelector<HTMLButtonElement>('.git-review__diff-card-main')
    if (!firstToggle) throw new Error('Review navigation fixture did not render a file toggle')
    firstToggle.click()
    await expect.poll(() => firstCard.dataset.expanded).toBe('true')

    await screen.rerender(renderPanel(2, 'src/target.ts'))
    await expect.poll(() => targetCard.dataset.expanded).toBe('true')
    expect(firstCard.dataset.expanded).toBe('true')
  })

  it('leaves ordinary last-turn review state unchanged when the path is absent', async () => {
    setTargetSpy.mockClear()
    const renderPanel = (requestId: number, filePath?: string) => (
      <GitReviewPanel
        isActive
        onOpenFile={NOOP}
        projectId="project-1"
        targetNavigation={{
          requestId,
          target: { kind: 'lastTurn', conversationId: 'conversation-1' },
          ...(filePath ? { filePath } : {})
        }}
      />
    )
    const screen = await render(renderPanel(1))

    const cards = Array.from(
      screen.container.querySelectorAll<HTMLElement>('.git-review__diff-card')
    )
    expect(cards).toHaveLength(2)
    const firstToggle = cards[0]?.querySelector<HTMLButtonElement>('.git-review__diff-card-main')
    if (!firstToggle) throw new Error('Review navigation fixture did not render a file toggle')
    firstToggle.click()
    await expect.poll(() => cards[0]?.dataset.expanded).toBe('true')

    await screen.rerender(renderPanel(2, 'src/missing.ts'))
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    expect(cards[0]?.dataset.expanded).toBe('true')
    expect(cards[1]?.dataset.expanded).toBeUndefined()
  })
})
