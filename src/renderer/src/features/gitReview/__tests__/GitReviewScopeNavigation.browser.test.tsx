import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { GitReviewScope } from '@mycopilot/protocol'
import '../GitReviewPanel.css'

const { setScopeSpy } = vi.hoisted(() => ({
  setScopeSpy: vi.fn()
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
    useGitReview: (
      _projectId: string,
      _isActive: boolean,
      _conversationId?: string | null,
      initialScope: GitReviewScope = 'unstaged'
    ) => {
      const [scope, setScopeState] = useState<GitReviewScope>(initialScope)
      const setScope = useCallback((nextScope: GitReviewScope) => {
        setScopeSpy(nextScope)
        setScopeState(nextScope)
      }, [])
      const summaryState = useMemo(
        () => ({
          status: 'ready' as const,
          value: {
            files,
            repositoryId: 'repository-1',
            scope,
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
        [scope]
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
        scope,
        setHotDiffFileIds: noop,
        setHotFullContentFileIds: noop,
        setScope,
        summaryState
      }
    }
  }
})

const { GitReviewPanel } = await import('../GitReviewPanel')
const NOOP = (): void => undefined

describe('Git Review scope navigation', () => {
  it('switches an existing review page to last turn once', async () => {
    setScopeSpy.mockClear()
    const screen = await render(<GitReviewPanel isActive onOpenFile={NOOP} projectId="project-1" />)

    await expect
      .element(screen.getByRole('button', { name: 'gitReview.scope.unstaged' }))
      .toBeVisible()
    await screen.rerender(
      <GitReviewPanel
        isActive
        onOpenFile={NOOP}
        projectId="project-1"
        scopeNavigation={{ requestId: 1, scope: 'lastTurn' }}
      />
    )

    await expect
      .element(screen.getByRole('button', { name: 'gitReview.scope.lastTurn' }))
      .toBeVisible()
    expect(setScopeSpy.mock.calls).toEqual([['lastTurn']])
  })

  it('keeps expanded state when an already-open last-turn page is activated again', async () => {
    setScopeSpy.mockClear()
    const renderPanel = (requestId: number) => (
      <GitReviewPanel
        isActive
        onOpenFile={NOOP}
        projectId="project-1"
        scopeNavigation={{ requestId, scope: 'lastTurn' }}
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
    expect(setScopeSpy).not.toHaveBeenCalled()
  })
})
