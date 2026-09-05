import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ReactNode } from 'react'
import { render as renderComponent } from 'vitest-browser-react'
import { ToastProvider } from '../../../components/toast/ToastProvider'
import '../../../styles/global.css'
import '../GitReviewPanel.css'

const { copyFilePathSpy, loadFileContentSpy } = vi.hoisted(() => ({
  copyFilePathSpy: vi.fn(),
  loadFileContentSpy: vi.fn()
}))
const render = (component: ReactNode) => renderComponent(component, { wrapper: ToastProvider })

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'en-US',
    resolvedThemeId: 'classic-light',
    t: (key: string) => key
  })
}))

vi.mock('../gitReviewClient', () => ({
  copyGitReviewFilePath: copyFilePathSpy,
  getGitReviewRepositoryContext: vi.fn(),
  listGitReviewCommits: vi.fn()
}))

vi.mock('../useGitReview', async () => {
  const { useCallback, useEffect, useRef, useState } = await import('react')
  const files = ['first', 'second', 'third'].map((name) => ({
    id: name,
    path: `src/${name}.ts`,
    ...(name === 'first' ? { previousPath: 'src/old-first.ts' } : {}),
    stats: { additions: 1, deletions: 0 },
    status: name === 'first' ? ('renamed' as const) : ('modified' as const)
  }))
  const summary = {
    files: [
      ...files,
      {
        id: 'binary-image',
        path: 'assets/preview.png',
        status: 'added' as const
      }
    ],
    context: {},
    repositoryId: 'repository-1',
    target: { kind: 'unstaged' },
    snapshotId: 'snapshot-1',
    stats: {
      additions: 999,
      deletions: 888,
      fileCount: files.length + 1,
      lineCountsComplete: false
    },
    truncated: false
  }
  const noop = (): void => undefined
  const noopAsync = async (): Promise<void> => undefined
  const emptyFileContentStates = {}

  return {
    gitReviewTargetKey: (target: { kind: string }) => target.kind,
    useGitReview: () => {
      const [diffStates, setDiffStates] = useState<Record<string, object>>({})
      const diffStatesRef = useRef(diffStates)
      const timersRef = useRef<number[]>([])
      useEffect(
        () => () => {
          timersRef.current.forEach((timer) => window.clearTimeout(timer))
        },
        []
      )
      const loadFileDiff = useCallback(async (fileId: string): Promise<void> => {
        const existing = diffStatesRef.current[fileId] as { status?: string } | undefined
        if (existing?.status === 'loading' || existing?.status === 'ready') return
        diffStatesRef.current = {
          ...diffStatesRef.current,
          [fileId]: { status: 'loading' }
        }
        setDiffStates(diffStatesRef.current)
        const timer = window.setTimeout(() => {
          const lineCount = 48
          const patch = [
            `@@ -1,${lineCount} +1,${lineCount} @@`,
            ...Array.from({ length: lineCount }, (_, index) => ` ${fileId} line ${index + 1}`)
          ].join('\n')
          diffStatesRef.current = {
            ...diffStatesRef.current,
            [fileId]: {
              status: 'ready',
              value: {
                fileId,
                patch,
                snapshotId: 'snapshot-1',
                status: 'ready'
              }
            }
          }
          setDiffStates(diffStatesRef.current)
        }, 30)
        timersRef.current.push(timer)
      }, [])

      return {
        cancelQueuedFileContentsExcept: noop,
        cancelQueuedFileDiffsExcept: noop,
        diffStates,
        dismissMutationError: noop,
        fileContentStates: emptyFileContentStates,
        loadFileContent: loadFileContentSpy,
        loadFileDiff,
        mutateFile: noopAsync,
        mutationError: null,
        pendingFileId: null,
        refresh: noopAsync,
        retryFileDiff: loadFileDiff,
        setHotDiffFileIds: noop,
        setHotFullContentFileIds: noop,
        setTarget: noop,
        target: { kind: 'unstaged' as const },
        summaryState: { status: 'ready', value: summary }
      }
    }
  }
})

const { GitReviewPanel } = await import('../GitReviewPanel')
const openFile = (): void => undefined

beforeEach(() => {
  copyFilePathSpy.mockReset().mockResolvedValue(undefined)
  loadFileContentSpy.mockClear()
  window.localStorage.removeItem('mycopilot.gitReview.preferences.v1')
})

describe('GitReviewPanel interactions', () => {
  it('copies a renamed file using its current path and current project', async () => {
    const onOpenFile = vi.fn()
    const screen = await render(
      <div style={{ height: 500, width: 440 }}>
        <GitReviewPanel isActive onOpenFile={onOpenFile} projectId="project-1" />
      </div>
    )
    const clickFirstCopyButton = (): void => {
      const button = screen.container.querySelector<HTMLButtonElement>(
        '.git-review__diff-card[data-file-id="first"] [aria-label="gitReview.file.copyPath"]'
      )
      if (!button) throw new Error('Copy action did not render')
      button.click()
    }

    clickFirstCopyButton()

    await expect.poll(() => copyFilePathSpy.mock.calls.length).toBe(1)
    expect(copyFilePathSpy).toHaveBeenLastCalledWith({
      projectId: 'project-1',
      path: 'src/first.ts'
    })
    expect(onOpenFile).not.toHaveBeenCalled()
    expect(
      screen.container.querySelector('.git-review__diff-card[data-expanded="true"]')
    ).toBeNull()
    expect(loadFileContentSpy).not.toHaveBeenCalled()

    await screen.rerender(
      <div style={{ height: 500, width: 440 }}>
        <GitReviewPanel isActive onOpenFile={onOpenFile} projectId="project-2" />
      </div>
    )
    clickFirstCopyButton()

    await expect.poll(() => copyFilePathSpy.mock.calls.length).toBe(2)
    expect(copyFilePathSpy).toHaveBeenLastCalledWith({
      projectId: 'project-2',
      path: 'src/first.ts'
    })
  })

  it('reports a rejected host file path copy without opening or expanding the file', async () => {
    copyFilePathSpy.mockRejectedValueOnce(new Error('File does not exist'))
    const onOpenFile = vi.fn()
    const screen = await render(
      <div style={{ height: 500, width: 440 }}>
        <GitReviewPanel isActive onOpenFile={onOpenFile} projectId="project-1" />
      </div>
    )
    const copyButton = screen.container.querySelector<HTMLButtonElement>(
      '.git-review__diff-card[data-file-id="first"] [aria-label="gitReview.file.copyPath"]'
    )
    if (!copyButton) throw new Error('Copy action did not render')

    copyButton.click()

    await expect.element(screen.getByRole('status')).toHaveTextContent('gitReview.copy.failed')
    expect(onOpenFile).not.toHaveBeenCalled()
    expect(
      screen.container.querySelector('.git-review__diff-card[data-expanded="true"]')
    ).toBeNull()
  })

  it('shows and totals only files with concrete line statistics', async () => {
    const screen = await render(
      <div style={{ height: 500, width: 440 }}>
        <GitReviewPanel isActive onOpenFile={openFile} projectId="project-1" />
      </div>
    )

    expect(screen.container.querySelectorAll('.git-review__diff-card')).toHaveLength(3)
    expect(
      screen.container.querySelector('.git-review__toolbar')?.hasAttribute('data-has-context')
    ).toBe(false)
    expect(screen.container.textContent).not.toContain('assets/preview.png')
    expect(screen.container.querySelector('.git-review__file-count')?.textContent).toBe('3')
    expect(screen.container.querySelector('.git-review__line-stats')?.textContent).toBe('+3-0')
    expect(screen.container.querySelector('.git-review__line-stats')?.textContent).not.toContain(
      '?'
    )
  })

  it('shows every file type while totaling only concrete line statistics', async () => {
    const screen = await render(
      <div style={{ height: 500, width: 440 }}>
        <GitReviewPanel isActive onOpenFile={openFile} projectId="project-1" />
      </div>
    )

    await screen.getByRole('button', { name: 'gitReview.options' }).click()
    await screen.getByRole('menuitem', { name: 'gitReview.showAllFileTypes' }).click()

    await expect
      .poll(() => screen.container.querySelectorAll('.git-review__diff-card').length)
      .toBe(4)
    expect(screen.container.textContent).toContain('assets/preview.png')
    expect(screen.container.querySelector('.git-review__file-count')?.textContent).toBe('4')
    expect(screen.container.querySelector('.git-review__line-stats')?.textContent).toBe('+3-0')
    expect(screen.container.querySelector('.git-review__line-stats')?.textContent).not.toContain(
      '?'
    )
    expect(window.localStorage.getItem('mycopilot.gitReview.preferences.v1')).toBe(
      JSON.stringify({ loadFullFiles: true, showAllFileTypes: true })
    )

    await screen.getByRole('button', { name: 'gitReview.options' }).click()
    await screen.getByRole('menuitem', { name: 'gitReview.dontShowAllFileTypes' }).click()

    await expect
      .poll(() => screen.container.querySelectorAll('.git-review__diff-card').length)
      .toBe(3)
    expect(screen.container.textContent).not.toContain('assets/preview.png')
    expect(screen.container.querySelector('.git-review__file-count')?.textContent).toBe('3')
    expect(screen.container.querySelector('.git-review__line-stats')?.textContent).toBe('+3-0')
    expect(window.localStorage.getItem('mycopilot.gitReview.preferences.v1')).toBe(
      JSON.stringify({ loadFullFiles: true, showAllFileTypes: false })
    )
  })

  it('persists the load-full-files action inside the review module', async () => {
    const screen = await render(
      <div style={{ height: 300, width: 440 }}>
        <GitReviewPanel isActive onOpenFile={openFile} projectId="project-1" />
      </div>
    )

    await screen.getByRole('button', { name: 'gitReview.options' }).click()
    await screen.getByRole('menuitem', { name: 'gitReview.dontLoadFullFiles' }).click()
    expect(window.localStorage.getItem('mycopilot.gitReview.preferences.v1')).toBe(
      JSON.stringify({ loadFullFiles: false, showAllFileTypes: false })
    )

    await screen.getByRole('button', { name: 'gitReview.options' }).click()
    await expect
      .element(screen.getByRole('menuitem', { name: 'gitReview.loadFullFiles' }))
      .toBeVisible()
  })

  it('loads full content only after an expanded file enters the review scrollport', async () => {
    const screen = await render(
      <div style={{ height: 120, width: 440 }}>
        <GitReviewPanel isActive onOpenFile={openFile} projectId="project-1" />
      </div>
    )
    const thirdToggle = screen.container.querySelector<HTMLButtonElement>(
      '.git-review__diff-card[data-file-id="third"] .git-review__diff-card-main'
    )
    const content = screen.container.querySelector<HTMLElement>('.git-review__content')
    const thirdWrapper = screen.container.querySelector<HTMLElement>(
      '[data-review-file-id="third"]'
    )
    if (!thirdToggle || !content || !thirdWrapper)
      throw new Error('Visibility fixture did not render')

    thirdWrapper.style.marginTop = '500px'
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    thirdToggle.click()
    await expect
      .poll(
        () =>
          screen.container.querySelectorAll(
            '.git-review__diff-card[data-file-id="third"] .git-review__unified-diff'
          ).length
      )
      .toBe(1)
    expect(loadFileContentSpy).not.toHaveBeenCalled()

    content.scrollTop = thirdWrapper.offsetTop
    content.dispatchEvent(new Event('scroll'))
    await expect.poll(() => loadFileContentSpy.mock.calls.length).toBe(1)
    expect(loadFileContentSpy).toHaveBeenCalledWith('third')
  })

  it('does not load full content while the review module is inactive', async () => {
    const screen = await render(
      <div style={{ height: 300, width: 440 }}>
        <GitReviewPanel isActive={false} onOpenFile={openFile} projectId="project-1" />
      </div>
    )
    const firstToggle = screen.container.querySelector<HTMLButtonElement>(
      '.git-review__diff-card[data-file-id="first"] .git-review__diff-card-main'
    )
    if (!firstToggle) throw new Error('Inactive fixture did not render')
    firstToggle.click()
    await new Promise((resolve) => window.setTimeout(resolve, 70))
    expect(loadFileContentSpy).not.toHaveBeenCalled()
  })

  it('honors the module-owned load-full-files preference', async () => {
    window.localStorage.setItem(
      'mycopilot.gitReview.preferences.v1',
      JSON.stringify({ loadFullFiles: false })
    )
    const screen = await render(
      <div style={{ height: 300, width: 440 }}>
        <GitReviewPanel isActive onOpenFile={openFile} projectId="project-1" />
      </div>
    )
    const firstToggle = screen.container.querySelector<HTMLButtonElement>(
      '.git-review__diff-card[data-file-id="first"] .git-review__diff-card-main'
    )
    if (!firstToggle) throw new Error('Preference fixture did not render')
    firstToggle.click()
    await new Promise((resolve) => window.setTimeout(resolve, 70))
    expect(loadFileContentSpy).not.toHaveBeenCalled()
  })

  it('keeps the action label and layout icon aligned with the target mode', async () => {
    const screen = await render(
      <div style={{ height: 500, width: 440 }}>
        <GitReviewPanel isActive onOpenFile={openFile} projectId="project-1" />
      </div>
    )
    const switchToSplit = screen.getByRole('button', {
      name: 'gitReview.view.switchSplit'
    })
    const splitButton = switchToSplit.element()

    expect(splitButton.getAttribute('aria-pressed')).toBeNull()
    expect(splitButton.getAttribute('data-active')).toBeNull()
    expect(splitButton.querySelector('.git-review__layout-icon')?.getAttribute('data-mode')).toBe(
      'split'
    )

    await switchToSplit.click()

    const switchToUnified = screen.getByRole('button', {
      name: 'gitReview.view.switchUnified'
    })
    expect(
      switchToUnified.element().querySelector('.git-review__layout-icon')?.getAttribute('data-mode')
    ).toBe('unified')
  })

  it('realigns a selected file after its lazy diff increases the scroll range', async () => {
    const screen = await render(
      <div style={{ height: 500, width: 440 }}>
        <GitReviewPanel isActive onOpenFile={openFile} projectId="project-1" />
      </div>
    )

    await screen.getByRole('button', { name: 'gitReview.files.show' }).click()
    const fileItems = [
      ...screen.container.querySelectorAll<HTMLButtonElement>('.git-review__file-list-item')
    ]
    const targetItem = fileItems.at(-1)
    if (!targetItem) throw new Error('File list fixture did not render')
    targetItem.click()

    await expect
      .poll(() => screen.container.querySelectorAll('.git-review__unified-diff').length)
      .toBe(1)

    const content = screen.container.querySelector<HTMLElement>('.git-review__content')
    const targetHeader = screen.container.querySelector<HTMLElement>(
      '.git-review__diff-card[data-file-id="third"] .git-review__diff-card-header'
    )
    if (!content || !targetHeader) throw new Error('Selected file fixture did not render')

    expect(
      Math.abs(targetHeader.getBoundingClientRect().top - content.getBoundingClientRect().top)
    ).toBeLessThanOrEqual(1)
  })

  it('stops native smooth scrolling when the user starts interacting with the diff', async () => {
    const screen = await render(
      <div style={{ height: 500, width: 440 }}>
        <GitReviewPanel isActive onOpenFile={openFile} projectId="project-1" />
      </div>
    )

    await screen.getByRole('button', { name: 'gitReview.files.show' }).click()
    const targetItem = [
      ...screen.container.querySelectorAll<HTMLButtonElement>('.git-review__file-list-item')
    ].at(-1)
    if (!targetItem) throw new Error('File list fixture did not render')
    targetItem.click()
    await expect
      .poll(() => screen.container.querySelectorAll('.git-review__unified-diff').length)
      .toBe(1)

    const content = screen.container.querySelector<HTMLElement>('.git-review__content')
    if (!content) throw new Error('Review scroll container did not render')
    content.scrollTop = 0
    content.scrollTo({ behavior: 'smooth', top: content.scrollHeight })
    await expect.poll(() => content.scrollTop).toBeGreaterThan(0)

    content.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'mouse' }))
    await nextFrames(2)
    const stoppedAt = content.scrollTop
    await new Promise((resolve) => window.setTimeout(resolve, 200))

    expect(Math.abs(content.scrollTop - stoppedAt)).toBeLessThanOrEqual(1)
  })
})

async function nextFrames(count: number): Promise<void> {
  for (let index = 0; index < count; index += 1) {
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
  }
}
