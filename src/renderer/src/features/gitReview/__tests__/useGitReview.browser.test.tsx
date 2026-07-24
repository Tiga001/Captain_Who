import type {
  GitReviewFileContent,
  GitReviewFileDiff,
  GitReviewScope,
  GitReviewSummary
} from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const { contentSpy, diffSpy, mutateSpy, summarySpy } = vi.hoisted(() => ({
  contentSpy: vi.fn(),
  diffSpy: vi.fn(),
  mutateSpy: vi.fn(),
  summarySpy: vi.fn()
}))

vi.mock('../gitReviewClient', () => ({
  getGitReviewFileContent: contentSpy,
  getGitReviewFileDiff: diffSpy,
  getGitReviewSummary: summarySpy,
  mutateGitReviewFile: mutateSpy
}))

const { useGitReview } = await import('../useGitReview')

function summary(snapshotId: string): GitReviewSummary {
  return {
    files: [
      {
        id: 'file-1',
        path: 'src/file.ts',
        stats: { additions: 1, deletions: 0 },
        status: 'modified'
      }
    ],
    repositoryId: 'repository-1',
    scope: 'unstaged',
    snapshotId,
    stats: {
      additions: 1,
      deletions: 0,
      fileCount: 1,
      lineCountsComplete: true
    },
    truncated: false
  }
}

function readyDiff(snapshotId = 'snapshot-1'): GitReviewFileDiff {
  return {
    fileId: 'file-1',
    patch: '@@ -1 +1 @@\n-old\n+new',
    snapshotId,
    status: 'ready'
  }
}

function readyContent(snapshotId = 'snapshot-1'): GitReviewFileContent {
  return {
    afterText: 'new',
    beforeText: 'old',
    fileId: 'file-1',
    snapshotId,
    status: 'ready'
  }
}

function ReviewHarness({
  active,
  initialScope
}: {
  active: boolean
  initialScope?: GitReviewScope
}) {
  const review = useGitReview('project-1', active, 'conversation-1', initialScope)
  return (
    <div>
      <output data-testid="scope">{review.scope}</output>
      <output data-testid="summary-status">
        {review.summaryState.status}:{review.summaryState.value?.snapshotId ?? 'none'}:
        {review.summaryState.status === 'error' ? review.summaryState.error : 'ok'}
      </output>
      <output data-testid="diff-status">{review.diffStates['file-1']?.status ?? 'idle'}</output>
      <output data-testid="content-status">
        {review.fileContentStates['file-1']?.status ?? 'idle'}
      </output>
      <button type="button" onClick={() => review.loadFileDiff('file-1', 'high')}>
        load diff
      </button>
      <button type="button" onClick={() => review.loadFileContent('file-1')}>
        load content
      </button>
      <button type="button" onClick={() => review.retryFileDiff('file-1')}>
        retry diff
      </button>
      <button type="button" onClick={() => void review.refresh()}>
        refresh
      </button>
      <button type="button" onClick={() => review.setScope('lastTurn')}>
        last turn
      </button>
      <button type="button" onClick={() => void review.mutateFile('file-1', 'stage')}>
        mutate
      </button>
    </div>
  )
}

beforeEach(() => {
  contentSpy.mockReset()
  diffSpy.mockReset()
  mutateSpy.mockReset()
  summarySpy.mockReset()
  summarySpy.mockResolvedValue(summary('snapshot-1'))
  diffSpy.mockResolvedValue(readyDiff())
  contentSpy.mockResolvedValue(readyContent())
})

describe('useGitReview request lifecycle', () => {
  it('uses a requested last-turn scope for the first review request', async () => {
    const screen = await render(<ReviewHarness active initialScope="lastTurn" />)

    await expect.element(screen.getByTestId('scope')).toHaveTextContent('lastTurn')
    await expect.poll(() => summarySpy.mock.calls.length).toBe(1)
    expect(summarySpy).toHaveBeenCalledWith({
      conversationId: 'conversation-1',
      projectId: 'project-1',
      scope: 'lastTurn'
    })
  })

  it('keeps the last ready review usable when foreground refresh fails', async () => {
    const screen = await render(<ReviewHarness active />)
    await expect
      .element(screen.getByTestId('summary-status'))
      .toHaveTextContent('ready:snapshot-1:ok')

    await screen.getByRole('button', { name: 'load diff' }).click()
    await expect.element(screen.getByTestId('diff-status')).toHaveTextContent('ready')

    await screen.rerender(<ReviewHarness active={false} />)
    summarySpy.mockRejectedValueOnce(new Error('refresh failed'))
    await screen.rerender(<ReviewHarness active />)

    await expect
      .element(screen.getByTestId('summary-status'))
      .toHaveTextContent('error:snapshot-1:refresh failed')
    await expect.element(screen.getByTestId('diff-status')).toHaveTextContent('ready')

    summarySpy.mockResolvedValueOnce(summary('snapshot-2'))
    await screen.getByRole('button', { name: 'refresh' }).click()
    await expect
      .element(screen.getByTestId('summary-status'))
      .toHaveTextContent('ready:snapshot-2:ok')
    await expect.element(screen.getByTestId('diff-status')).toHaveTextContent('idle')
  })

  it('clears orphaned loading state when running content finishes in the background', async () => {
    const pendingContent = deferred<GitReviewFileContent>()
    contentSpy.mockReset()
    contentSpy.mockReturnValueOnce(pendingContent.promise).mockResolvedValueOnce(readyContent())
    const screen = await render(<ReviewHarness active />)
    await expect
      .element(screen.getByTestId('summary-status'))
      .toHaveTextContent('ready:snapshot-1:ok')

    await screen.getByRole('button', { name: 'load content' }).click()
    await expect.element(screen.getByTestId('content-status')).toHaveTextContent('loading')
    expect(contentSpy).toHaveBeenCalledTimes(1)

    await screen.rerender(<ReviewHarness active={false} />)
    pendingContent.resolve(readyContent())
    await expect.element(screen.getByTestId('content-status')).toHaveTextContent('idle')

    summarySpy.mockRejectedValueOnce(new Error('refresh failed'))
    await screen.rerender(<ReviewHarness active />)
    await expect
      .element(screen.getByTestId('summary-status'))
      .toHaveTextContent('error:snapshot-1:refresh failed')

    await screen.getByRole('button', { name: 'load content' }).click()
    await expect.element(screen.getByTestId('content-status')).toHaveTextContent('ready')
    expect(contentSpy).toHaveBeenCalledTimes(2)
  })

  it('turns an expired snapshot into an explicit retry state without looping', async () => {
    const screen = await render(<ReviewHarness active />)
    await expect
      .element(screen.getByTestId('summary-status'))
      .toHaveTextContent('ready:snapshot-1:ok')
    diffSpy
      .mockResolvedValueOnce({
        fileId: 'file-1',
        snapshotId: 'snapshot-1',
        status: 'snapshotExpired'
      } satisfies GitReviewFileDiff)
      .mockResolvedValueOnce(readyDiff())
    summarySpy.mockRejectedValueOnce(new Error('refresh failed'))

    await screen.getByRole('button', { name: 'load diff' }).click()
    await expect.element(screen.getByTestId('diff-status')).toHaveTextContent('error')
    await expect
      .element(screen.getByTestId('summary-status'))
      .toHaveTextContent('error:snapshot-1:refresh failed')
    expect(diffSpy).toHaveBeenCalledTimes(1)

    await screen.getByRole('button', { name: 'load diff' }).click()
    expect(diffSpy).toHaveBeenCalledTimes(1)

    await screen.getByRole('button', { name: 'retry diff' }).click()
    await expect.element(screen.getByTestId('diff-status')).toHaveTextContent('ready')
    expect(diffSpy).toHaveBeenCalledTimes(2)
  })

  it('scopes last-turn reads to the active conversation and blocks mutations', async () => {
    const screen = await render(<ReviewHarness active />)
    await expect
      .element(screen.getByTestId('summary-status'))
      .toHaveTextContent('ready:snapshot-1:ok')

    await screen.getByRole('button', { name: 'last turn' }).click()
    await expect.poll(() => summarySpy.mock.calls.length).toBeGreaterThan(1)
    expect(summarySpy).toHaveBeenLastCalledWith({
      conversationId: 'conversation-1',
      projectId: 'project-1',
      scope: 'lastTurn'
    })

    await screen.getByRole('button', { name: 'mutate' }).click()
    expect(mutateSpy).not.toHaveBeenCalled()
  })
})

function deferred<T>(): {
  promise: Promise<T>
  resolve: (value: T) => void
} {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}
