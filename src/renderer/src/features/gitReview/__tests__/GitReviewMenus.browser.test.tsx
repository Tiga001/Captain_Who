import { useState, type ReactNode } from 'react'
import type {
  GitReviewBranch,
  GitReviewCommit,
  GitReviewContext,
  GitReviewRepositoryContext,
  GitReviewTarget
} from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { Translate } from '../../../config/translationFormat'
import { ToastProvider } from '../../../components/toast/ToastProvider'
import '../../../styles/global.css'
import { GitReviewBranchPicker } from '../GitReviewBranchPicker'
import { GitReviewContextRow } from '../GitReviewContextRow'
import { GitReviewSourceSelector } from '../GitReviewSourceSelector'
import '../GitReviewPanel.css'

const { copyTextSpy, listCommitsSpy } = vi.hoisted(() => ({
  copyTextSpy: vi.fn<(text: string) => Promise<void>>(),
  listCommitsSpy: vi.fn()
}))

vi.mock('../../../components/clipboard', () => ({
  copyTextToClipboard: copyTextSpy
}))

vi.mock('../gitReviewClient', () => ({
  inspectGitRepository: vi.fn(),
  listGitReviewCommits: listCommitsSpy
}))

const t: Translate = (key) => key
const commit: GitReviewCommit = {
  committedAt: '2026-09-05T00:00:00.000Z',
  message: 'Add committed review source',
  parents: ['parent-a'],
  sha: 'abcdef1234567890',
  stats: {
    additions: 4,
    deletions: 2,
    fileCount: 2,
    lineCountsComplete: true
  },
  subject: 'Add committed review source'
}
const repositoryContext: GitReviewRepositoryContext = {
  branches: [
    { isDefault: true, kind: 'remote', name: 'origin/main', ref: 'refs/remotes/origin/main' },
    {
      isDefault: false,
      kind: 'remote',
      name: 'origin/release/long-running-review-work',
      ref: 'refs/remotes/origin/release/long-running-review-work'
    }
  ],
  currentBranch: 'feature/review-sources',
  defaultBaseRef: 'refs/remotes/origin/main',
  headSha: 'head-a',
  repositoryId: 'repository-1',
  truncated: false
}

function SourceSelectorHarness(): ReactNode {
  const [target, setTarget] = useState<GitReviewTarget>({ kind: 'uncommitted' })
  const [isOpen, setIsOpen] = useState(false)
  return (
    <div className="git-review" style={{ height: 300, width: 440 }}>
      <GitReviewSourceSelector
        branchBaseRef="refs/remotes/origin/main"
        conversationId="conversation-1"
        fileCount={3}
        isOpen={isOpen}
        language="en-US"
        onOpenChange={setIsOpen}
        onRequestRepositoryContext={vi.fn()}
        onSelectBranch={() => setTarget({ kind: 'branch', baseRef: 'refs/remotes/origin/main' })}
        onSelectCommit={(selected) => setTarget({ kind: 'commit', commitSha: selected.sha })}
        onSelectTarget={setTarget}
        projectId="project-1"
        repositoryState={{ status: 'ready', value: repositoryContext }}
        t={t}
        target={target}
      />
      <output data-testid="selected-target">
        {target.kind === 'commit'
          ? `${target.kind}:${target.commitSha}`
          : target.kind === 'branch'
            ? `${target.kind}:${target.baseRef}`
            : target.kind}
      </output>
    </div>
  )
}

function dispatchKey(target: Element, key: string): void {
  target.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key }))
}

beforeEach(() => {
  copyTextSpy.mockReset()
  copyTextSpy.mockResolvedValue(undefined)
  listCommitsSpy.mockReset()
  listCommitsSpy.mockResolvedValue({
    commits: [commit],
    repositoryId: 'repository-1',
    truncated: false
  })
})

describe('Git review source menus', () => {
  it('portals the grouped sources in the required order and returns focus on Escape', async () => {
    const screen = await render(<SourceSelectorHarness />)
    const trigger = screen.getByRole('button', { name: 'gitReview.scope.uncommitted' })

    await trigger.click()
    const menu = screen.getByRole('menu', { name: 'gitReview.source.menu' }).element()
    expect(menu.parentElement).toBe(document.body)
    expect(
      Array.from(menu.querySelectorAll<HTMLElement>(':scope > [role="menuitemradio"]')).map(
        (item) => item.textContent?.trim()
      )
    ).toEqual([
      'gitReview.scope.lastTurn',
      'gitReview.scope.uncommitted',
      'gitReview.scope.unstaged',
      'gitReview.scope.staged',
      'gitReview.scope.committed',
      'gitReview.scope.branch'
    ])
    expect(menu.querySelectorAll(':scope > [role="separator"]')).toHaveLength(2)

    const selectedItem = menu.querySelector<HTMLElement>('[aria-checked="true"]')
    if (!selectedItem) throw new Error('Selected review source did not render')
    selectedItem.focus()
    dispatchKey(selectedItem, 'ArrowDown')
    expect(document.activeElement?.textContent?.trim()).toBe('gitReview.scope.unstaged')
    dispatchKey(document.activeElement as Element, 'ArrowUp')
    expect(document.activeElement).toBe(selectedItem)
    dispatchKey(selectedItem, 'Escape')

    await expect
      .poll(() => document.querySelector('[aria-label="gitReview.source.menu"]'))
      .toBeNull()
    await expect.poll(() => document.activeElement).toBe(trigger.element())
  })

  it('lazy-loads the committed submenu and selects a local commit', async () => {
    const screen = await render(<SourceSelectorHarness />)
    await screen.getByRole('button', { name: 'gitReview.scope.uncommitted' }).click()

    expect(listCommitsSpy).not.toHaveBeenCalled()
    await screen.getByRole('menuitemradio', { name: /gitReview\.scope\.committed/ }).click()
    await expect.poll(() => listCommitsSpy.mock.calls.length).toBe(1)

    const submenu = screen.getByRole('menu', { name: 'gitReview.commit.menu' }).element()
    expect(submenu.parentElement).toBe(document.body)
    expect(submenu.textContent).toContain('Add committed review source')
    expect(submenu.textContent).toContain('+4')
    expect(submenu.textContent).toContain('-2')

    await screen.getByRole('menuitemradio', { name: /Add committed review source/ }).click()
    await expect
      .element(screen.getByTestId('selected-target'))
      .toHaveTextContent('commit:abcdef1234567890')
    expect(document.querySelector('[aria-label="gitReview.commit.menu"]')).toBeNull()
    expect(document.querySelector('[aria-label="gitReview.source.menu"]')).toBeNull()
  })

  it('opens and closes the commit submenu with horizontal arrow keys', async () => {
    const commitsRequest = deferred<{
      commits: GitReviewCommit[]
      repositoryId: string
      truncated: boolean
    }>()
    listCommitsSpy.mockReset()
    listCommitsSpy.mockReturnValue(commitsRequest.promise)
    const screen = await render(<SourceSelectorHarness />)
    await screen.getByRole('button', { name: 'gitReview.scope.uncommitted' }).click()
    await expect
      .poll(() => document.activeElement?.textContent?.trim())
      .toBe('gitReview.scope.uncommitted')
    const committed = screen
      .getByRole('menuitemradio', { name: /gitReview\.scope\.committed/ })
      .element()
    committed.focus()
    dispatchKey(committed, 'ArrowRight')

    await expect.poll(() => listCommitsSpy.mock.calls.length).toBe(1)
    await expect
      .poll(() => document.querySelector<HTMLElement>('[aria-label="gitReview.commit.menu"]'))
      .not.toBeNull()
    const submenu = document.querySelector<HTMLElement>('[aria-label="gitReview.commit.menu"]')
    if (!submenu) throw new Error('Commit submenu did not render')
    expect(submenu.textContent).toContain('gitReview.commit.loading')
    commitsRequest.resolve({
      commits: [commit],
      repositoryId: 'repository-1',
      truncated: false
    })
    await expect.poll(() => submenu.contains(document.activeElement)).toBe(true)
    dispatchKey(document.activeElement as Element, 'ArrowLeft')

    await expect
      .poll(() => document.querySelector('[aria-label="gitReview.commit.menu"]'))
      .toBeNull()
    await expect.poll(() => document.activeElement).toBe(committed)
  })
})

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}

describe('Git review branch picker', () => {
  it('searches branch candidates in a Portal and switches the selected base', async () => {
    const onSelect = vi.fn<(branch: GitReviewBranch) => void>()
    const screen = await render(
      <div className="git-review" style={{ height: 220, width: 440 }}>
        <GitReviewBranchPicker
          isActive
          onRetry={vi.fn()}
          onSelect={onSelect}
          selectedRef="refs/remotes/origin/main"
          state={{ status: 'ready', value: repositoryContext }}
          t={t}
        />
      </div>
    )
    const trigger = screen.getByRole('button', { name: 'gitReview.branch.select' })

    await trigger.click()
    const menu = screen.getByRole('menu', { name: 'gitReview.branch.select' }).element()
    expect(menu.parentElement).toBe(document.body)
    await screen.getByRole('searchbox', { name: 'gitReview.branch.search' }).fill('release')
    await expect
      .element(screen.getByRole('menuitemradio', { name: /origin\/release/ }))
      .toBeVisible()
    expect(menu.querySelectorAll('[role="menuitemradio"]')).toHaveLength(1)

    await screen.getByRole('menuitemradio', { name: /origin\/release/ }).click()
    expect(onSelect).toHaveBeenCalledWith(repositoryContext.branches[1])
    await expect.poll(() => document.activeElement).toBe(trigger.element())
  })

  it('closes from the search field with Escape and restores trigger focus', async () => {
    const screen = await render(
      <div className="git-review" style={{ height: 220, width: 440 }}>
        <GitReviewBranchPicker
          isActive
          onRetry={vi.fn()}
          onSelect={vi.fn()}
          selectedRef="refs/remotes/origin/main"
          state={{ status: 'ready', value: repositoryContext }}
          t={t}
        />
      </div>
    )
    const trigger = screen.getByRole('button', { name: 'gitReview.branch.select' })
    await trigger.click()
    const search = screen.getByRole('searchbox', { name: 'gitReview.branch.search' }).element()
    dispatchKey(search, 'Escape')

    await expect
      .poll(() => document.querySelector('[aria-label="gitReview.branch.select"][role="menu"]'))
      .toBeNull()
    await expect.poll(() => document.activeElement).toBe(trigger.element())
  })
})

describe('Git review context rows', () => {
  it('shows commit and branch context only for their new review targets', async () => {
    const commitScreen = await render(<CommitContextFixture commitPreview={commit} />)
    expect(commitScreen.container.textContent).toContain(commit.subject)
    await commitScreen.unmount()

    const branchScreen = await render(
      <div className="git-review">
        <GitReviewContextRow
          isActive
          language="en-US"
          onRetryBranches={vi.fn()}
          onSelectBranch={vi.fn()}
          repositoryState={{ status: 'ready', value: repositoryContext }}
          t={t}
          target={{ kind: 'branch', baseRef: 'refs/remotes/origin/main' }}
        />
      </div>
    )
    expect(branchScreen.container.textContent).toContain('feature/review-sources')
    await expect
      .element(branchScreen.getByRole('button', { name: 'gitReview.branch.select' }))
      .toBeVisible()
  })

  it('copies every line from the commit tooltip with the same localized formatting', async () => {
    const screen = await render(<CommitContextFixture commitPreview={commit} language="zh-CN" />)
    await screen.getByText(commit.subject, { exact: true }).hover()
    await expect.element(screen.getByRole('tooltip')).toBeVisible()
    const tooltip = screen.getByRole('tooltip').element()
    const lines = Array.from(
      tooltip.querySelectorAll('.git-review__commit-tooltip > *'),
      (line) => line.textContent
    )
    expect(lines).toEqual([
      commit.subject,
      commit.sha.slice(0, 12),
      new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short' }).format(
        Date.parse(commit.committedAt)
      ),
      '+4 -2'
    ])

    await screen.getByRole('button', { name: 'gitReview.commit.copyInfo' }).click()
    expect(copyTextSpy).toHaveBeenCalledExactlyOnceWith(lines.join('\n'))
  })

  it('briefly reports clipboard failure and allows retrying', async () => {
    copyTextSpy.mockRejectedValueOnce(new Error('Clipboard unavailable'))
    const screen = await render(<CommitContextFixture commitPreview={commit} />)
    const copyButton = screen.getByRole('button', { name: 'gitReview.commit.copyInfo' })
    await copyButton.click()

    await expect.element(screen.getByRole('status')).toHaveTextContent('gitReview.copy.failed')
    await expect.poll(() => document.querySelector('.toast-message'), { timeout: 3000 }).toBeNull()
    await copyButton.click()
    expect(copyTextSpy).toHaveBeenCalledTimes(2)
  })

  it('disables copying while details are loading and never copies an old selected commit', async () => {
    const screen = await render(<CommitContextFixture />)
    const copyButton = screen.getByRole('button', { name: 'gitReview.commit.copyInfo' })
    await expect.element(copyButton).toBeDisabled()

    await screen.rerender(<CommitContextFixture commitPreview={commit} />)
    await expect.element(copyButton).toBeEnabled()
    await copyButton.click()
    expect(copyTextSpy).toHaveBeenCalledTimes(1)

    const nextCommit = {
      ...commit,
      sha: '123456abcdef7890',
      subject: 'Review the next commit'
    }
    const target: GitReviewTarget = { kind: 'commit', commitSha: nextCommit.sha }
    await screen.rerender(
      <CommitContextFixture commitPreview={commit} summaryContext={{ commit }} target={target} />
    )
    await expect.element(copyButton).toBeDisabled()
    expect(screen.container.textContent).not.toContain(commit.subject)
    expect(copyTextSpy).toHaveBeenCalledTimes(1)

    await screen.rerender(
      <CommitContextFixture
        commitPreview={nextCommit}
        summaryContext={{ commit }}
        target={target}
      />
    )
    await expect.element(copyButton).toBeEnabled()
    await copyButton.click()
    expect(copyTextSpy).toHaveBeenCalledTimes(2)
    expect(copyTextSpy.mock.calls[1]?.[0]).toMatch(/^Review the next commit\n123456abcdef\n/)
  })
})

function CommitContextFixture({
  commitPreview,
  language = 'en-US',
  summaryContext,
  target = { kind: 'commit', commitSha: commit.sha }
}: {
  commitPreview?: GitReviewCommit
  language?: string
  summaryContext?: GitReviewContext
  target?: GitReviewTarget
}): ReactNode {
  return (
    <ToastProvider>
      <div className="git-review" style={{ width: 440 }}>
        <GitReviewContextRow
          commitPreview={commitPreview}
          isActive
          language={language}
          onRetryBranches={vi.fn()}
          onSelectBranch={vi.fn()}
          repositoryState={{ status: 'idle' }}
          summaryContext={summaryContext}
          t={t}
          target={target}
        />
      </div>
    </ToastProvider>
  )
}
